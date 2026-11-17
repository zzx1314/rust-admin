#ifndef YOLOV8_DETECTOR_H
#define YOLOV8_DETECTOR_H

#include <string>
#include <vector>
#include <cstdint>
#include "rknn_api.h"
#include <opencv2/opencv.hpp>

// 单个检测结果
struct Detection {
    int cls_id;            // 类别 id
    float score;           // 置信度 (0~1)
    cv::Rect box;          // 原始图像坐标系下的框
};

// YOLOv8 (RKNN fp16) 检测器封装
class YoloV8Detector {
public:
    YoloV8Detector();
    ~YoloV8Detector();

    // 初始化：加载 .rknn 模型与 labels.txt
    //   rknn_path : 模型路径
    //   label_path: 类别名文本(每行一个)，为空时自动按 0..nc-1 编号
    //   conf      : 置信度阈值
    //   nms       : NMS IoU 阈值
    //   model_in  : 模型输入边长(默认 640)
    int init(const std::string& rknn_path, const std::string& label_path,
             float conf = 0.25f, float nms = 0.45f, int model_in = 640);

    // 释放当前 RKNN 上下文（--serve 模型热切换用）。
    // init() 开头会自动调用，因此重复 init 是安全的；卸载后 ctx_ 归零。
    void release();

    // 对一张 BGR 图像推理，返回检测结果(原始图像坐标)
    int detect(const cv::Mat& bgr, std::vector<Detection>& results);

    // 在图像上画框与标签
    void draw(cv::Mat& bgr, const std::vector<Detection>& results);

    int num_classes() const { return (int)labels_.size(); }
    const std::string& class_name(int id) const;
    // 运行时改置信度阈值（--serve 的 CONF 命令用），不重载模型
    void set_conf(float c) { conf_ = c; }

    // 模型属性（只读，供 --json 输出与「模型包契约校验」使用）
    int in_type() const { return in_type_; }
    int in_fmt() const { return in_fmt_; }
    int input_size() const { return model_in_; }
    uint32_t out_n_elems() const { return out_n_elems_; }

private:
    // 字母盒缩放，返回缩放比例/填充，并把图处理成 model_in_ x model_in_ 的 CHW float  blob
    cv::Mat letterbox(const cv::Mat& bgr, float& scale, int& pad_x, int& pad_y);
    void blobFromImage(const cv::Mat& rgb_pad, std::vector<float>& blob);  // /255 -> CHW (planar)
    uint16_t float32_to_float16(float v);

    int postprocess(const float* out, int n_elems, int channels, int anchors,
                    float scale, int pad_x, int pad_y,
                    const cv::Size& orig, std::vector<Detection>& results);
    static float iou(const cv::Rect& a, const cv::Rect& b);
    void nms(std::vector<Detection>& dets, float thr);

    rknn_context ctx_ = 0;          // rknn_context
    int model_in_ = 640;
    float conf_ = 0.25f;
    float nms_ = 0.45f;
    int in_type_ = 0;              // RKNN_TENSOR_FLOAT32 / FLOAT16
    int in_fmt_ = 0;              // RKNN_TENSOR_NCHW / NHWC (必须与模型声明一致)
    bool in_pass_through_ = false;
    uint32_t out_n_elems_ = 0;     // 输出张量元素数，= (4+nc)*8400
    std::vector<std::string> labels_;
};

#endif  // YOLOV8_DETECTOR_H
