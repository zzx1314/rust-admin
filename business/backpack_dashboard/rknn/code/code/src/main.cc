#include "yolov8_detector.h"
#include <opencv2/opencv.hpp>
#include <iostream>
#include <fstream>
#include <sstream>
#include <chrono>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

static std::string try_paths(const std::vector<std::string>& candidates) {
    for (auto& p : candidates) {
        std::ifstream f(p);
        if (f.good()) return p;
    }
    return candidates.empty() ? "" : candidates.back();
}

// JSON 字符串转义：标签名/路径可能含引号、反斜杠或控制字符
static std::string json_escape(const std::string& s) {
    std::ostringstream o;
    for (unsigned char c : s) {
        switch (c) {
            case '"':  o << "\\\""; break;
            case '\\': o << "\\\\"; break;
            case '\b': o << "\\b"; break;
            case '\f': o << "\\f"; break;
            case '\n': o << "\\n"; break;
            case '\r': o << "\\r"; break;
            case '\t': o << "\\t"; break;
            default:
                if (c < 0x20) {
                    char buf[8];
                    snprintf(buf, sizeof(buf), "\\u%04x", c);
                    o << buf;
                } else {
                    o << (char)c;
                }
        }
    }
    return o.str();
}

// 模型契约信息（阶段 1 懒加载校验要用：n_elems == (4+nc)*8400 / NHWC / FP16）
static std::string model_json(const YoloV8Detector& det) {
    std::ostringstream o;
    o << "{\"nc\":" << det.num_classes()
      << ",\"n_elems\":" << det.out_n_elems()
      << ",\"in_fmt\":\"" << json_escape(get_format_string((rknn_tensor_format)det.in_fmt())) << "\""
      << ",\"in_type\":\"" << json_escape(get_type_string((rknn_tensor_type)det.in_type())) << "\""
      << ",\"imgsz\":" << det.input_size() << "}";
    return o.str();
}

// 一次推理的完整 JSON（--json 与 --serve 的 DETECT 共用）
// extra: serve 模式下追加 "cmd"/"id"/"ok" 字段，--json 时为空，保证阶段 0 输出格式不变。
static std::string result_json(const YoloV8Detector& det, const std::string& task,
                               const std::string& input, const cv::Mat& img,
                               const std::vector<Detection>& results, long infer_ms,
                               const std::string& extra = "") {
    std::ostringstream o;
    o << "{";
    if (!extra.empty()) o << extra << ",";
    o << "\"task\":\"" << json_escape(task) << "\""
      << ",\"input\":\"" << json_escape(input) << "\""
      << ",\"model\":" << model_json(det)
      << ",\"frame\":{\"w\":" << img.cols << ",\"h\":" << img.rows << "}"
      << ",\"count\":" << results.size()
      << ",\"infer_ms\":" << infer_ms
      << ",\"boxes\":[";
    for (size_t i = 0; i < results.size(); i++) {
        const auto& r = results[i];
        if (i) o << ",";
        o << "{\"cls_id\":" << r.cls_id
          << ",\"label\":\"" << json_escape(det.class_name(r.cls_id)) << "\""
          << ",\"score\":" << r.score
          << ",\"x1\":" << r.box.x
          << ",\"y1\":" << r.box.y
          << ",\"x2\":" << (r.box.x + r.box.width)
          << ",\"y2\":" << (r.box.y + r.box.height) << "}";
    }
    o << "]}";
    return o.str();
}

struct ModelPaths { std::string model, labels; };

int main(int argc, char** argv) {
    std::string task, model, labels, input, output, models_root;
    bool json_out = false, serve = false, conf_set = false;
    float conf = 0.25f, nms = 0.45f;
    int model_in = 640;

    for (int i = 1; i < argc; i++) {
        std::string a = argv[i];
        auto next = [&](const char* def) -> std::string {
            return (i + 1 < argc) ? std::string(argv[++i]) : std::string(def);
        };
        if (a == "--json")            json_out = true;      // 开关，不消耗下一个参数
        else if (a == "--serve")      serve = true;         // 常驻推理服务（stdin/stdout 行协议）
        else if (a == "--task")       task = next("");
        else if (a == "--model")      model = next("");
        else if (a == "--labels")     labels = next("");
        else if (a == "--input")      input = next("");
        else if (a == "--output")     output = next("");
        else if (a == "--models-root") models_root = next("");
        else if (a == "--conf")       { conf = std::stof(next("0.25")); conf_set = true; }
        else if (a == "--nms")        nms = std::stof(next("0.45"));
        else if (a == "--imgsz")      model_in = std::stoi(next("640"));
    }

    if (input.empty() && !serve) {
        std::cerr << "用法:\n"
                  << "  --task   <任务名>       按 models/<task>/ 自动定位模型与标签\n"
                  << "  --model  <model.rknn>  --labels <labels.txt>   显式指定模型\n"
                  << "  --input  <image>      (CLI 模式必填) 输入图片\n"
                  << "  --output <out.png>    (可选) 结果图保存路径\n"
                  << "  --conf   <0.25>     置信度阈值\n"
                  << "  --nms    <0.45>      NMS IoU 阈值\n"
                  << "  --imgsz  <640>       模型输入边长\n"
                  << "  --json               stdout 只输出一行结构化 JSON（供平台集成）\n"
                  << "  --serve              常驻模式：stdin 收命令，stdout 每行一条 JSON\n"
                  << "  --models-root <dir>  模型根目录（默认自动查找 rknn/models/）\n";
        return 1;
    }

    // 由可执行文件位置推导模型根目录：build/ -> code/code -> code -> rknn
    // 这样无论从 build/ 还是 code/code/ 运行，都能找到 rknn/models/<task>/。
    std::string exe_dir = ".";
    if (argc > 0) {
        std::string a0 = argv[0];
        size_t p = a0.find_last_of("/\\");
        if (p != std::string::npos) exe_dir = a0.substr(0, p);
    }

    std::vector<std::string> roots;
    if (!models_root.empty()) roots.push_back(models_root);
    roots.push_back(exe_dir + "/../../../models");             // build/ 运行时
    roots.push_back(exe_dir + "/../../models");                // 直接在 code/code/ 下运行时
    roots.push_back("models");                                 // 兼容原有 CWD 相对路径
    roots.push_back("/root/data/yolo_model_code/code/models"); // 兼容旧板端路径

    // 解析模型与 labels 路径（--serve 的 SWITCH 会复用这个 lambda）
    auto resolve = [&](const std::string& t) -> ModelPaths {
        ModelPaths p;
        p.model = model;
        p.labels = labels;
        if (p.model.empty()) {
            std::vector<std::string> cands;
            for (const auto& r : roots) cands.push_back(r + "/" + t + "/model.rknn");
            p.model = try_paths(cands);
        }
        if (p.labels.empty()) {
            std::vector<std::string> cands;
            for (const auto& r : roots) cands.push_back(r + "/" + t + "/labels.txt");
            p.labels = try_paths(cands);
        }
        return p;
    };

    // serve 模式把 C++ 内部阈值调低：只做粗筛，精确阈值由后端按用户设置过滤，
    // 这样网页上调阈值无需重新加载模型。显式给过 --conf 则尊重用户取值。
    if (serve && !conf_set) conf = 0.15f;

    if (task.empty() && model.empty()) {
        std::cerr << "错误: 必须指定 --task 或 --model\n";
        return 1;
    }
    ModelPaths p0 = resolve(task);

    YoloV8Detector det;
    if (det.init(p0.model, p0.labels, conf, nms, model_in) != 0) {
        std::cerr << "初始化失败: " << p0.model << "\n";
        if (serve) std::cout << "{\"cmd\":\"READY\",\"ok\":false,\"error\":\"模型初始化失败\"}" << std::endl;
        return 1;
    }

    // ------------------ 常驻推理服务 ------------------
    // 行协议（stdin 一行命令 -> stdout 恰好一行 JSON；所有诊断走 stderr）：
    //   PING                        -> 心跳/当前模型信息
    //   SWITCH <task>               -> 卸载当前模型并加载新场景
    //   DETECT <id> <image_path>    -> 对一张图推理，回一行 JSON
    //   CONF <float>                -> 运行时改置信度（不重载模型）
    //   QUIT                        -> 退出
    // 路径与 task 名不含空格（平台使用固定 /tmp 目录与目录名）。
    if (serve) {
        std::cout << "{\"cmd\":\"READY\",\"ok\":true,\"task\":\"" << json_escape(task)
                  << "\",\"model\":" << model_json(det) << "}" << std::endl;
        std::string line;
        while (std::getline(std::cin, line)) {
            if (!line.empty() && line.back() == '\r') line.pop_back();
            std::istringstream iss(line);
            std::string cmd;
            if (!(iss >> cmd)) continue;

            if (cmd == "QUIT") {
                break;
            } else if (cmd == "PING") {
                std::cout << "{\"cmd\":\"PING\",\"ok\":true,\"task\":\"" << json_escape(task)
                          << "\",\"model\":" << model_json(det) << "}" << std::endl;
            } else if (cmd == "CONF") {
                float c = 0.25f;
                if (iss >> c && c >= 0.0f && c <= 1.0f) {
                    det.set_conf(c);
                    std::cout << "{\"cmd\":\"CONF\",\"ok\":true,\"conf\":" << c << "}" << std::endl;
                } else {
                    std::cout << "{\"cmd\":\"CONF\",\"ok\":false,\"error\":\"conf 需为 0~1\"}" << std::endl;
                }
            } else if (cmd == "SWITCH") {
                std::string t;
                if (!(iss >> t)) {
                    std::cout << "{\"cmd\":\"SWITCH\",\"ok\":false,\"error\":\"缺少 task\"}" << std::endl;
                    continue;
                }
                ModelPaths p = resolve(t);
                std::ifstream chk(p.model);
                if (!chk.good()) {
                    std::cout << "{\"cmd\":\"SWITCH\",\"ok\":false,\"task\":\"" << json_escape(t)
                              << "\",\"error\":\"找不到 model.rknn\"}" << std::endl;
                    continue;
                }
                if (det.init(p.model, p.labels, conf, nms, model_in) != 0) {
                    std::cout << "{\"cmd\":\"SWITCH\",\"ok\":false,\"task\":\"" << json_escape(t)
                              << "\",\"error\":\"模型加载失败，当前模型已卸载\"}" << std::endl;
                    continue;
                }
                task = t;
                std::cout << "{\"cmd\":\"SWITCH\",\"ok\":true,\"task\":\"" << json_escape(task)
                          << "\",\"model\":" << model_json(det) << "}" << std::endl;
            } else if (cmd == "DETECT") {
                long id = 0;
                std::string path;
                if (!(iss >> id >> path)) {
                    std::cout << "{\"cmd\":\"DETECT\",\"ok\":false,\"error\":\"用法 DETECT <id> <path>\"}" << std::endl;
                    continue;
                }
                cv::Mat img = cv::imread(path);
                if (img.empty()) {
                    std::cout << "{\"cmd\":\"DETECT\",\"id\":" << id
                              << ",\"ok\":false,\"error\":\"无法读取图片\"}" << std::endl;
                    continue;
                }
                std::vector<Detection> results;
                auto t0 = std::chrono::steady_clock::now();
                int rc = det.detect(img, results);
                auto t1 = std::chrono::steady_clock::now();
                long ms = (long)std::chrono::duration_cast<std::chrono::milliseconds>(t1 - t0).count();
                if (rc != 0) {
                    std::cout << "{\"cmd\":\"DETECT\",\"id\":" << id
                              << ",\"ok\":false,\"error\":\"推理失败\"}" << std::endl;
                    continue;
                }
                std::ostringstream ex;
                ex << "\"cmd\":\"DETECT\",\"id\":" << id << ",\"ok\":true";
                std::cout << result_json(det, task, path, img, results, ms, ex.str()) << std::endl;
            } else {
                std::cout << "{\"cmd\":\"" << json_escape(cmd)
                          << "\",\"ok\":false,\"error\":\"未知命令\"}" << std::endl;
            }
        }
        return 0;
    }

    // ------------------ 原 CLI 模式（语义与阶段 0 完全一致） ------------------
    cv::Mat img = cv::imread(input);
    if (img.empty()) {
        std::cerr << "无法读取图片: " << input << "\n";
        return 1;
    }

    std::vector<Detection> results;
    auto t0 = std::chrono::steady_clock::now();
    int rc = det.detect(img, results);
    auto t1 = std::chrono::steady_clock::now();
    long infer_ms = (long)std::chrono::duration_cast<std::chrono::milliseconds>(t1 - t0).count();
    if (rc != 0) {
        std::cerr << "推理失败\n";
        return 1;
    }

    if (json_out) {
        // stdout 严格只输出一行 JSON；所有诊断信息在 detector 内部走 stderr。
        // 坐标由 cv::Rect(x,y,w,h) 转成 x1/y1/x2/y2，与后续 SSE detect 事件一致。
        std::cout << result_json(det, task, input, img, results, infer_ms) << std::endl;
    } else {
        std::cout << "检测到 " << results.size() << " 个目标:\n";
        for (const auto& r : results) {
            std::cout << "  [" << det.class_name(r.cls_id) << "] score=" << r.score
                      << " box=(" << r.box.x << "," << r.box.y << ","
                      << r.box.width << "," << r.box.height << ")\n";
        }
    }

    // 原 CLI 语义保留：默认仍会画框并写出 result_<task>.png。
    // --json 且未指定 --output 时不产生任何文件/额外 stdout，避免副作用污染一行 JSON。
    if (!output.empty() || !json_out) {
        det.draw(img, results);
        std::string out_path = output.empty() ? ("result_" + task + ".png") : output;
        cv::imwrite(out_path, img);
        if (!json_out) std::cout << "结果已保存: " << out_path << "\n";
    }
    return 0;
}
