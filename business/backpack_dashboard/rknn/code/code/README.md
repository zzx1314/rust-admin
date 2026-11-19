# YOLOv8 目标检测 RKNN 部署工程说明（RK3576）

本工程把 6 个 YOLOv8s 检测模型转换为 RKNN（FP16，不量化，target=`rk3576`），
并提供一套统一的 C++ 推理程序。模型由 ultralytics 训练，部署在 RK3576 开发板（aarch64）。

---

## 1. 工程概览

| 任务(task) | 场景 | 类别数 | 输出张量维度 |
|---|---|---|---|
| `overflow`  | 垃圾桶溢出 / 状态识别 | 5  | `[1, 9, 8400]`  |
| `fall`      | 跌倒检测             | 1  | `[1, 5, 8400]`  |
| `fire`      | 火灾检测             | 4  | `[1, 8, 8400]`  |
| `dog_leash` | 牵犬绳检测           | 2  | `[1, 6, 8400]`  |
| `smoke`     | 烟雾检测             | 1  | `[1, 5, 8400]`  |
| `fire-lane` | 消防车道/禁停区监测  | 5  | `[1, 9, 8400]`  |

板端目录约定（`/root/data/yolo_model_code/code/`）：

```
code/
├── CMakeLists.txt
├── include/rknn_api.h
├── src/
│   ├── main.cc              # CLI 入口：解析参数 + 自动定位模型/labels
│   ├── yolov8_detector.h
│   └── yolov8_detector.cc   # 推理封装：预处理 / RKNN 推理 / 解码 / NMS / 画框
├── models/<task>/model.rknn # 转换后的模型
│            └── labels.txt  # 类别名（每行一个）
├── build/yolov8_detector    # 编译产物
├── convert_all.py           # 模型下载→ONNX→RKNN 转换脚本（板端运行）
└── README.md
```

---

## 2. 模型简要说明

> 类别名取自各 `best.pt` 的 `model.names`，转换时由 `convert_all.py` 自动写入 `labels.txt`。

- **overflow（垃圾桶状态，5 类）**
  `0:Broken trash can`（破损）, `1:Healthy trash can`（完好）, `2:Trash can closed`（已盖）,
  `3:Trash can open`（开盖）, `4:Trash over flow`（溢出）。
- **fall（跌倒，1 类）**
  `0:fall`。
- **fire（火灾，4 类）**
  `0:------------------------------`（占位/无关）, `1:fire`, `2:fire - v1 2023-10-05 11-48am`,
  `3:fire - v2 2024-06-19 3-12pm`。注意类 0 为占位名，**实际火灾一般在类 1~3 中出高分**，
  使用时可按需忽略类 0 或合并类 1~3。
- **dog_leash（犬绳，2 类）**
  `0:leash-detect`（有绳）, `1:leash-detect-not`（无绳）。这是一个二分类检测头，两类都会被检出；
  若只关心"有绳"可只保留类 0。
- **smoke（烟雾，1 类）**
  `0:smoke`。单类检测，实测对真实烟雾柱（楼群间、逆光场景）能给出 0.8+ 的高置信框。
- **fire-lane（消防车道监测，5 类）**
  `0:Barricade`（路障/挡车器）, `1:Car`（车辆）, `2:ElectricBike`（电动车）,
  `3:FirelaneArea`（消防车道区域）, `4:Person`（人员）。数据来自 Roboflow 项目
  `fire-lane-monitor`（v8），模型权重 `runs/yolov8s/fire-lane/weights/best.pt`。

---

## 3. 输入 / 输出维度与数据结构

### 3.1 输入
- 张量形状：`[1, 640, 640, 3]`，**FP16 / NHWC**，RGB 顺序，数值范围 **0~1**（已做 `/255`）。
- 预处理流程（见 `yolov8_detector.cc::letterbox` + `blobFromImage`）：
  1. 等比例缩放（letterbox），短边对齐 640，长边按比例，不足部分用 **114** 灰度填充居中；
  2. BGR → RGB；
  3. 每个像素 `/255.0`。
- ⚠️ **关键约束**：模型声明输入布局为 **NHWC**，C++ 必须按 NHWC 组装 FP16 输入字节并以
  `pass_through=1` 直传（见 `detect()` 中 `in_fmt_ == RKNN_TENSOR_NHWC` 分支）。若误按 NCHW
  直传，NPU 会把张量读错，导致检测分数整体漂移（曾出现 dog_leash 误判为 0、fire 出现假阳性）。

### 3.2 输出
- 单输出张量，形状 `[1, 4+nc, 8400]`，**FP16 / UNDEFINED**，C++ 取 `want_float=1` 自动转 float32。
- 内存布局为 **channel-major**（与 ultralytics 导出一致）：
  - 前 4 个通道 = 每个锚点的框 `[cx, cy, w, h]`，坐标为 **输入 640 空间下的像素值**（已含锚点偏移，**无需 DFL 解码**）；
  - 后 `nc` 个通道 = 每个锚点各类别的 **概率**（sigmoid 已在导出图中激活，**无需再 sigmoid**）。
  - `8400 = 80×80 + 40×40 + 20×20`（三个检测尺度锚点数之和）。
- 解码公式（见 `detect()`）：
  ```
  cx, cy, w, h = out[0..3 * A + a]      # A = 8400
  best_cls, best_score  = argmax/ max over out[(4+c)*A + a]
  if best_score > conf: 保留
  # 反 letterbox 回原始图像坐标
  x1 = (cx - w/2 - pad_x) / scale ;  ...  → cv::Rect
  ```
  随后做按类的 NMS（IoU 阈值 `nms_`），最后画框。

---

## 4. 程序使用方法

### 4.1 编译（在 RK3576 板端）
```bash
cd /root/data/yolo_model_code/code
mkdir -p build && cd build
cmake .. && make
# 产物: build/yolov8_detector
```
依赖：OpenCV（板端已装，`find_package(OpenCV)`）、`librknnrt.so`（`/usr/lib`）、`rknn_api.h`（`/usr/include`）。
工程自带 `include/rknn_api.h` 作为兜底，CMake 优先用系统头。

### 4.2 运行
```bash
./build/yolov8_detector --task <task> --input <image> \
    [--output out.jpg] [--conf 0.25] [--nms 0.45] \
    [--labels models/<task>/labels.txt] [--model models/<task>/model.rknn]
```

| 参数 | 说明 | 默认 |
|---|---|---|
| `--task`   | 任务名：`overflow`/`fall`/`fire`/`dog_leash`/`smoke`/`fire-lane` | 必填 |
| `--input`  | 输入图像路径（BGR，jpg/png 均可）            | 必填 |
| `--output` | 画框结果保存路径；省略则不落盘只打印          | 空   |
| `--conf`   | 置信度阈值                                    | 0.25 |
| `--nms`    | NMS IoU 阈值                                  | 0.45 |
| `--labels` | 类别名文件；省略时按 `models/<task>/labels.txt` | 自动 |
| `--model`  | 模型文件；省略时按 `models/<task>/model.rknn`  | 自动 |

路径解析：`--labels` / `--model` 省略时，程序用 `try_paths()` 在
`models/<task>/` 下自动查找 `model.rknn` 与 `labels.txt`，无需每次手填。

### 4.3 示例
```bash
# 检测犬绳并保存结果图
./build/yolov8_detector --task dog_leash \
    --input /root/data/yolo_model_code/datasets_unzip/dog_leash/valid/images/xxx.jpg \
    --output /tmp/res.jpg

# 调高阈值只保留高置信火灾
./build/yolov8_detector --task fire --input fire.jpg --conf 0.5
```

程序会先打印：模型输入/输出属性、类别数、输入边长、阈值，再打印每个检测框
（`[类别名] score=0.xx box=(x,y,w,h)` 原始坐标），最后若指定 `--output` 则保存画框图。

---

## 5. 模型下载与转换方法

### 5.1 原始权重与训练代码
- 训练框架源码：`/root/data/yolo_model_code/ultralytics-main`（ultralytics 8.3.9）。
- 训练产物（每个任务一份）：`/root/data/yolo_model_code/runs/yolov8s/<task>/weights/best.pt`。
- 数据集：`/root/data/yolo_model_code/datasets_unzip/<task>/`（train/valid 已解压，其中 **fall 未在板端解压**，仅余 dog_leash/fire/overflow）。
  `smoke` 未解压到 `datasets_unzip/`，但其原始数据在 `/root/data/yolo_model_code/datasets/smoke/`
  （含 `valid/images`、`train/images`），测试时直接用这里的真实图即可。
- `fire-lane` 数据集：`/root/data/yolo_model_code/datasets/Fire-Lane-Monitor.v8i.yolov8.zip`
  （Roboflow 导出，含 train 801 / valid 40 / test 21 张）。板端需先解压：
  `unzip -q Fire-Lane-Monitor.v8i.yolov8.zip -d Fire-Lane-Monitor`，解压后真实测试图在
  `datasets/Fire-Lane-Monitor/valid/images/`（40 张）与 `test/images/`（21 张）。

### 5.2 转换脚本 `convert_all.py`（在板端运行）
逐任务执行：`best.pt` → ONNX(640) → RKNN(FP16)。

关键步骤：
1. 加载模型并导出 ONNX：
   ```python
   model = YOLO('runs/yolov8s/<task>/weights/best.pt')
   model.export(format='onnx', imgsz=640, batch=1, dynamic=False, simplify=False)
   # 导出文件 best.onnx -> 重命名为 models/<task>/model.onnx
   ```
2. 转 RKNN（FP16，不量化）：
   ```python
   from rknn.api import RKNN
   rk = RKNN()
   rk.config(target_platform='rk3576')        # 注意：未烘焙 mean/std，输入为 FP16
   rk.load_onnx(model=onnx_path)
   rk.build(do_quantization=False)            # FP16
   rk.export_rknn('models/<task>/model.rknn')
   ```
3. 把 `model.names` 写入 `models/<task>/labels.txt`（每行一个类别名）。

> ⚠️ 板端无外网，`ultralytics` 导入依赖 `matplotlib` / `requests`。`convert_all.py`
> 在脚本内用内存桩（`sys.modules` 注入空 `matplotlib.pyplot` 与 `requests` 的
> `Response`/`Session` 等）绕过这两个依赖，无需 `pip install`。直接跑 `python3 convert_all.py` 即可。
>
> 新增任务：把任务名加进 `convert_all.py` 的 `TASKS` 列表后重跑；或参考板端现成的
> `convert_smoke.py` / `convert_firelane.py`（单任务版，逻辑完全一致）复制改名。
> `fire-lane` 即采用单任务脚本 `convert_firelane.py` 完成转换。C++ 侧无需改动——
> 推理程序按 `--task` 自动定位 `models/<task>/`，类别数由 `labels.txt` 行数决定。

### 5.3 转换后确认
- 输入：用 `rknn_query(RKNN_QUERY_INPUT_ATTR)` 确认 `FP16 / NHWC / [1,640,640,3]`。
- 输出：用 `RKNN_QUERY_OUTPUT_ATTR` 确认 `dims=[1, 4+nc, 8400]`、`n_elems=(4+nc)*8400`。
- 端到端一致性：本工程已用 Python ultralytics（`model.predict`）对 dog_leash / fire / overflow
  做交叉验证，C++ 输出与 Python ground truth 在坐标与分数上一致（误差来自 FP16 量化，<0.01）。

---

## 6. 批量测试与结果目录约定

工程提供一个批量测试流程（本地驱动脚本 `batch_test.py`，经 paramiko 在板端逐图跑
`yolov8_detector`）：每个任务取若干张测试图，推理结果按任务分目录落盘，便于横向对比。

- 板端结果根目录：`code/results/`
  - `results/<task>/res_<原图名>.jpg` —— 画框结果图
  - `results/<task>/<原图名>.txt` —— 该图的结构化检测报告（输入/输出属性 + 每个框的类别/分数/坐标）
  - `results/<task>/summary.txt` —— 该任务的统计（测试张数、总检出、各图检出数、类别分布）
  - `results/summary.txt` —— 6 个任务的总汇总
- 测试图来源：`datasets_unzip/<task>/valid/images/`（dog_leash/fire/overflow）、
  `datasets/smoke/valid/images/`（smoke）、`datasets/fall/valid/images/`（fall，1450 张原始数据）、
  `datasets/Fire-Lane-Monitor/valid/images/`（fire-lane，40 张真实验证图）。
- 最近一次批量（每任务 6 张）结果摘要见 `results/summary.txt`；`fire` 类 0 为占位名，应用层应过滤。

> ⚠️ 本地 Windows 驱动脚本拼接板端 Linux 路径时，**必须用正斜杠**手写拼接
> （如 `path.rstrip('/') + '/' + name`）。`os.path.join` 在 Windows 上会生成反斜杠，
> 传到板端后路径变成 `images\xxx.jpg` 被当成文件名的一部分 → "无法读取图片"，
> 且不会报错只会 0 检出，排查时务必检查路径分隔符。

## 7. 已知事项与注意事项

1. **输入布局必须 NHWC**：见 3.1。这是部署中最易踩的坑，已通过 `in_fmt_` 自适应修正。
2. **fire 类 0 是占位名**：实际火焰在类 1~3，应用层按需过滤类 0 或合并类 1~3。
3. **fall 任务板端无验证图**：`datasets_unzip` 未包含 fall 数据集，当前 `test_fall.jpg` 为合成冒烟图
   （仅验证程序不崩溃、能出图），**不表示 fall 模型精度**。正式验证需在板端放真实跌倒样本。
4. **dog_leash 二分类**：两类均会检出，若只关心"牵绳"请只取类 0（`leash-detect`）。
5. **FP16 精度**：相比原 PyTorch float32，分数误差约 ±0.01，对 0.25 阈值检出无实质影响。
6. **fire-lane 已用真实验证集交叉验证**：在 `datasets/Fire-Lane-Monitor/valid/images` 抽 6 张图，
   C++ 与 Python ultralytics（ONNX）的检出数量与类别分布**逐图完全一致**；模型输入/输出属性为
   `FP16/NHWC [1,640,640,3]` → `[1,9,8400]`，与通用解码器吻合，无需改 C++。

---

## 8. 快速验收记录（本次部署）

| 任务 | 测试图 | C++ 结果 | Python 交叉验证 |
|---|---|---|---|
| dog_leash | valid 样本 | 1 框 `leash-detect 0.69` | 一致（0.69） |
| fire      | valid 样本 | 0 框（阈值 0.25）          | 一致（0 框） |
| overflow  | valid 样本 | 3 框 `Trash over flow` 0.86/0.75/0.60 | 一致 |
| fall      | 合成图     | 0 框（图内无人）            | 冒烟通过 |
| smoke     | `datasets/smoke/valid/images/gas_img_4000103_*.jpg` | 1 框 `smoke 0.82` (288,233,97,155) | 一致（0.82，框吻合） |
| fire-lane | valid 真实图 6 张 | 共 10 框（Car×2, Person×1, FirelaneArea×7） | 完全一致（C++=Python，逐图匹配） |
