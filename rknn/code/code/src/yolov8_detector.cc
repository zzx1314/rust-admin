#include "yolov8_detector.h"
#include <cstdio>
#include <cstring>
#include <cstdlib>
#include <fstream>
#include <vector>
#include <algorithm>
#include <utility>
#include <rknn_api.h>

#include <opencv2/opencv.hpp>

static int read_file(const std::string& path, std::vector<uint8_t>& buf) {
    std::ifstream f(path, std::ios::binary);
    if (!f.is_open()) return -1;
    f.seekg(0, std::ios::end);
    std::streamoff sz = f.tellg();
    f.seekg(0, std::ios::beg);
    buf.resize((size_t)sz);
    if (sz > 0) f.read(reinterpret_cast<char*>(buf.data()), sz);
    return 0;
}

YoloV8Detector::YoloV8Detector() {}
YoloV8Detector::~YoloV8Detector() { release(); }

void YoloV8Detector::release() {
    if (ctx_) {
        rknn_destroy(ctx_);
        ctx_ = 0;
    }
    labels_.clear();
    out_n_elems_ = 0;
    in_type_ = 0;
    in_fmt_ = 0;
    in_pass_through_ = false;
}

const std::string& YoloV8Detector::class_name(int id) const {
    static const std::string empty = "";
    if (id >= 0 && id < (int)labels_.size()) return labels_[id];
    return empty;
}

static inline uint16_t f32_to_f16_bits(float value) {
    uint32_t x; memcpy(&x, &value, 4);
    uint32_t sign = x & 0x80000000u;
    uint32_t exp  = x & 0x7f800000u;
    uint32_t man  = x & 0x007fffffu;
    uint16_t h = (uint16_t)((sign >> 16) & 0x8000u);
    if (exp >= 0x47800000u) {            // inf / nan
        h |= 0x7c00u;
    } else if (exp <= 0x38000000u) {      // 过小 -> 0
        h = (uint16_t)((sign >> 16) & 0x8000u);
    } else {
        int32_t e = (int32_t)((exp >> 23) - 127 + 15);
        uint32_t m = man >> 13;
        h = (uint16_t)(((sign >> 16) & 0x8000u) | ((uint32_t)e << 10) | m);
    }
    return h;
}

uint16_t YoloV8Detector::float32_to_float16(float value) {
    return f32_to_f16_bits(value);
}

// 输入像素只有 0..255 共 256 种取值，归一化后的 FP16 位型可以预计算。
// 每帧可省掉 3*640*640 = 122 万次软件 float->half 位运算。
struct PixelHalfLut {
    uint16_t v[256];
    PixelHalfLut() {
        for (int i = 0; i < 256; ++i) v[i] = f32_to_f16_bits((float)i / 255.0f);
    }
};
static const PixelHalfLut kPixelHalfLut;

int YoloV8Detector::init(const std::string& rknn_path, const std::string& label_path,
                         float conf, float nms, int model_in) {
    // --serve 的 SWITCH 会在同一对象上再次 init：先卸载旧模型，避免 NPU 上下文泄漏。
    release();
    conf_ = conf; nms_ = nms; model_in_ = model_in;

    // 读类别名
    if (!label_path.empty()) {
        std::ifstream lf(label_path);
        std::string line;
        while (std::getline(lf, line)) {
            if (!line.empty() && line.back() == '\r') line.pop_back();
            // 与后端 detect.py::_read_labels 保持一致：跳过纯空白行，
            // 但保留类别名内部的空格（如 "dog leash" / "Broken trash can"）。
            // 否则两侧类别数不一致，框的类别会整体错位。
            if (line.find_first_not_of(" \t") != std::string::npos)
                labels_.push_back(line);
        }
    }

    // 读模型
    std::vector<uint8_t> model_buf;
    if (read_file(rknn_path, model_buf) != 0) {
        fprintf(stderr, "无法打开模型文件: %s\n", rknn_path.c_str());
        return -1;
    }

    int ret = rknn_init(&ctx_, model_buf.data(), (uint32_t)model_buf.size(), 0, nullptr);
    if (ret != 0) {
        fprintf(stderr, "rknn_init 失败: %d\n", ret);
        return -1;
    }

    // 查询输入属性
    // 注意：这些诊断信息一律写 stderr。--json 模式下 stdout 必须只含一行 JSON，
    // 否则上层 json.loads() 会被这些文本污染而失败。
    rknn_tensor_attr in_attr;
    memset(&in_attr, 0, sizeof(in_attr));
    in_attr.index = 0;
    ret = rknn_query(ctx_, RKNN_QUERY_INPUT_ATTR, &in_attr, sizeof(in_attr));
    if (ret != 0) {
        fprintf(stderr, "查询输入属性失败: %d\n", ret);
    } else {
        in_type_ = in_attr.type;
        in_fmt_ = in_attr.fmt;
        in_pass_through_ = (in_attr.type == RKNN_TENSOR_FLOAT16);
        fprintf(stderr, "模型输入: %s/%s dims=[", get_type_string(in_attr.type), get_format_string(in_attr.fmt));
        for (uint32_t i = 0; i < in_attr.n_dims; i++) fprintf(stderr, "%u%s", in_attr.dims[i], i+1<in_attr.n_dims?",":"");
        fprintf(stderr, "]\n");
    }

    // 查询输出属性
    rknn_tensor_attr out_attr;
    memset(&out_attr, 0, sizeof(out_attr));
    out_attr.index = 0;
    ret = rknn_query(ctx_, RKNN_QUERY_OUTPUT_ATTR, &out_attr, sizeof(out_attr));
    if (ret != 0) {
        fprintf(stderr, "查询输出属性失败: %d\n", ret);
    } else {
        out_n_elems_ = out_attr.n_elems;
        fprintf(stderr, "模型输出: %s/%s dims=[", get_type_string(out_attr.type), get_format_string(out_attr.fmt));
        for (uint32_t i = 0; i < out_attr.n_dims; i++) fprintf(stderr, "%u%s", out_attr.dims[i], i+1<out_attr.n_dims?",":"");
        fprintf(stderr, "] n_elems=%u\n", out_attr.n_elems);
    }

    fprintf(stderr, "类别数: %d  输入边长: %d  置信度阈值: %.2f  NMS: %.2f\n",
            (int)labels_.size(), model_in_, conf_, nms_);
    return 0;
}

cv::Mat YoloV8Detector::letterbox(const cv::Mat& bgr, float& scale, int& pad_x, int& pad_y) {
    int W = bgr.cols, H = bgr.rows;
    scale = std::min((float)model_in_ / (float)W, (float)model_in_ / (float)H);
    int nW = (int)std::round(W * scale);
    int nH = (int)std::round(H * scale);
    cv::Mat resized;
    cv::resize(bgr, resized, cv::Size(nW, nH), 0, 0, cv::INTER_LINEAR);
    pad_x = (model_in_ - nW) / 2;
    pad_y = (model_in_ - nH) / 2;
    cv::Mat out(cv::Size(model_in_, model_in_), CV_8UC3, cv::Scalar(114, 114, 114));
    resized.copyTo(out(cv::Rect(pad_x, pad_y, nW, nH)));
    return out;
}

void YoloV8Detector::blobFromImage(const cv::Mat& rgb_pad, std::vector<float>& blob) {
    blob.resize(3 * model_in_ * model_in_);
    int wh = model_in_ * model_in_;
    const cv::Mat float_mat; // not used
    // rgb_pad 是 CV_8UC3
    for (int y = 0; y < model_in_; y++) {
        for (int x = 0; x < model_in_; x++) {
            const cv::Vec3b& p = rgb_pad.at<cv::Vec3b>(y, x);
            // 注意 letterbox 输入是 BGR，这里转 RGB 并 /255
            int idx = y * model_in_ + x;
            blob[0 * wh + idx] = p[2] / 255.0f;  // R
            blob[1 * wh + idx] = p[1] / 255.0f;  // G
            blob[2 * wh + idx] = p[0] / 255.0f;  // B
        }
    }
}

int YoloV8Detector::detect(const cv::Mat& bgr, std::vector<Detection>& results) {
    results.clear();
    if (ctx_ == 0) return -1;

    float scale; int pad_x, pad_y;
    cv::Mat pad = letterbox(bgr, scale, pad_x, pad_y);

    // 组装输入
    rknn_input input;
    memset(&input, 0, sizeof(input));
    input.index = 0;
    std::vector<uint8_t> f16_buf;      // 通用路径：逐字节 FP16
    std::vector<uint16_t> f16_words;   // 快速路径：NHWC 交错 FP16
    std::vector<float> blob;

    // 板端实测模型为 FP16/NHWC + pass_through。把原来的
    // 「CHW float 生成 -> NHWC 重组 -> 逐元素转 FP16」三段 640x640 标量循环
    // 合成一遍：直接按像素读 BGR uint8，查表得 R/G/B 的 FP16，交错写 NHWC。
    // 像素只有 256 种取值，查表与逐元素转换逐位一致，不改变推理数值。
    // 这是纯预处理优化，rknn_run / 后处理 / 输出坐标完全不动。
    const bool fast_nhwc_fp16 =
        in_pass_through_ && in_fmt_ == RKNN_TENSOR_NHWC &&
        in_type_ == RKNN_TENSOR_FLOAT16 &&
        pad.isContinuous() && pad.type() == CV_8UC3 && pad.dims == 2;

    if (fast_nhwc_fp16) {
        const int wh = model_in_ * model_in_;
        f16_words.resize((size_t)wh * 3);
        const uint8_t* src = pad.ptr<uint8_t>(0);
        uint16_t* dst = f16_words.data();
        for (int i = 0; i < wh; ++i) {
            const uint8_t b = src[i * 3 + 0];
            const uint8_t g = src[i * 3 + 1];
            const uint8_t r = src[i * 3 + 2];
            dst[i * 3 + 0] = kPixelHalfLut.v[r];  // R
            dst[i * 3 + 1] = kPixelHalfLut.v[g];  // G
            dst[i * 3 + 2] = kPixelHalfLut.v[b];  // B
        }
        input.buf = f16_words.data();
        input.size = (uint32_t)(f16_words.size() * sizeof(uint16_t));
        input.type = RKNN_TENSOR_FLOAT16;
        input.fmt = (rknn_tensor_format)in_fmt_;
        input.pass_through = 1;
    } else {
        blobFromImage(pad, blob);
        if (in_pass_through_) {
            // 模型输入为 FP16：手动转 float16 并以 pass_through 直传。
            // 关键：blob 为 CHW(planar)，但模型声明的输入布局可能是 NHWC，
            // 必须按模型声明布局重组字节，否则 pass_through 时 NPU 会把布局读错。
            int wh = model_in_ * model_in_;
            if (in_fmt_ == RKNN_TENSOR_NHWC) {
                // 重组为 NHWC(逐像素 R,G,B 交错)
                std::vector<float> nhwc(blob.size());
                for (int y = 0; y < model_in_; y++) {
                    for (int x = 0; x < model_in_; x++) {
                        int px = (y * model_in_ + x) * 3;
                        int chw = y * model_in_ + x;
                        nhwc[px + 0] = blob[0 * wh + chw];
                        nhwc[px + 1] = blob[1 * wh + chw];
                        nhwc[px + 2] = blob[2 * wh + chw];
                    }
                }
                f16_buf.resize(nhwc.size() * 2);
                for (size_t i = 0; i < nhwc.size(); i++) {
                    uint16_t hv = float32_to_float16(nhwc[i]);
                    memcpy(&f16_buf[i*2], &hv, 2);
                }
            } else {
                f16_buf.resize(blob.size() * 2);
                for (size_t i = 0; i < blob.size(); i++) {
                    uint16_t hv = float32_to_float16(blob[i]);
                    memcpy(&f16_buf[i*2], &hv, 2);
                }
            }
            input.buf = f16_buf.data();
            input.size = (uint32_t)f16_buf.size();
            input.type = RKNN_TENSOR_FLOAT16;
            input.fmt = (rknn_tensor_format)in_fmt_;
            input.pass_through = 1;
        } else {
            input.buf = blob.data();
            input.size = (uint32_t)(blob.size() * sizeof(float));
            input.type = RKNN_TENSOR_FLOAT32;
            input.fmt = (rknn_tensor_format)in_fmt_;
            input.pass_through = 0;
        }
    }

    int ret = rknn_inputs_set(ctx_, 1, &input);
    if (ret != 0) { fprintf(stderr, "rknn_inputs_set 失败: %d\n", ret); return -1; }

    ret = rknn_run(ctx_, nullptr);
    if (ret != 0) { fprintf(stderr, "rknn_run 失败: %d\n", ret); return -1; }

    // 取输出 (want_float=1 -> 自动转 float32)
    rknn_output output;
    memset(&output, 0, sizeof(output));
    output.index = 0;
    output.want_float = 1;
    output.is_prealloc = 0;
    ret = rknn_outputs_get(ctx_, 1, &output, nullptr);
    if (ret != 0) { fprintf(stderr, "rknn_outputs_get 失败: %d\n", ret); return -1; }

    float* out = (float*)output.buf;
    uint32_t n_elems = output.size / sizeof(float);

    // 通道数（含 4 个框通道 + nc 类）
    int channels = (int)labels_.size() + 4;
    int anchors = (int)(n_elems / channels);
    if (anchors <= 0) { rknn_outputs_release(ctx_, 1, &output); return -1; }

    std::vector<Detection> dets;
    dets.reserve(anchors);
    for (int a = 0; a < anchors; a++) {
        float cx = out[0 * anchors + a];
        float cy = out[1 * anchors + a];
        float w  = out[2 * anchors + a];
        float h  = out[3 * anchors + a];
        // 最优类别
        int best = -1; float best_s = -1.0f;
        for (int c = 0; c < (int)labels_.size(); c++) {
            float s = out[(4 + c) * anchors + a];
            if (s > best_s) { best_s = s; best = c; }
        }
        if (best < 0 || best_s <= conf_) continue;
        float x1 = cx - w / 2.0f, y1 = cy - h / 2.0f;
        float x2 = cx + w / 2.0f, y2 = cy + h / 2.0f;
        // 反 letterbox -> 原始坐标
        float ox1 = (x1 - (float)pad_x) / scale;
        float oy1 = (y1 - (float)pad_y) / scale;
        float ox2 = (x2 - (float)pad_x) / scale;
        float oy2 = (y2 - (float)pad_y) / scale;
        int ix1 = (int)std::max(0.0f, std::round(ox1));
        int iy1 = (int)std::max(0.0f, std::round(oy1));
        int ix2 = (int)std::min((float)bgr.cols, std::round(ox2));
        int iy2 = (int)std::min((float)bgr.rows, std::round(oy2));
        int bw = ix2 - ix1, bh = iy2 - iy1;
        if (bw <= 0 || bh <= 0) continue;
        Detection d; d.cls_id = best; d.score = best_s; d.box = cv::Rect(ix1, iy1, bw, bh);
        dets.push_back(d);
    }

    nms(dets, nms_);
    results = std::move(dets);

    if (getenv("RKNN_DEBUG")) {
        std::vector<std::pair<float,int>> sc;
        sc.reserve(anchors);
        for (int a = 0; a < anchors; a++) {
            float bs = -1.0f;
            for (int c = 0; c < (int)labels_.size(); c++) {
                float s = out[(4 + c) * anchors + a];
                if (s > bs) bs = s;
            }
            sc.push_back({bs, a});
        }
        std::sort(sc.begin(), sc.end(), [](const std::pair<float,int>& x, const std::pair<float,int>& y){ return x.first > y.first; });
        fprintf(stderr, "DBG n_elems=%u channels=%d anchors=%d out_size=%u\n", n_elems, channels, anchors, output.size);
        for (int k = 0; k < 10 && k < (int)sc.size(); k++) {
            int a = sc[k].second;
            float cx = out[0*anchors + a], cy = out[1*anchors + a], w = out[2*anchors + a], h = out[3*anchors + a];
            int best = -1; float bst = -1.0f;
            for (int c = 0; c < (int)labels_.size(); c++) { float s = out[(4+c)*anchors + a]; if (s > bst) { bst = s; best = c; } }
            fprintf(stderr, "DBG a=%d cls=%d score=%.4f box=(%.1f,%.1f,%.1f,%.1f)\n", a, best, bst, cx, cy, w, h);
        }
    }

    rknn_outputs_release(ctx_, 1, &output);
    return 0;
}

float YoloV8Detector::iou(const cv::Rect& a, const cv::Rect& b) {
    int xx1 = std::max(a.x, b.x), yy1 = std::max(a.y, b.y);
    int xx2 = std::min(a.x + a.width, b.x + b.width);
    int yy2 = std::min(a.y + a.height, b.y + b.height);
    int iw = std::max(0, xx2 - xx1), ih = std::max(0, yy2 - yy1);
    int inter = iw * ih;
    int uni = a.width * a.height + b.width * b.height - inter;
    if (uni <= 0) return 0.f;
    return (float)inter / (float)uni;
}

void YoloV8Detector::nms(std::vector<Detection>& dets, float thr) {
    std::sort(dets.begin(), dets.end(), [](const Detection& a, const Detection& b){ return a.score > b.score; });
    std::vector<bool> keep(dets.size(), true);
    for (size_t i = 0; i < dets.size(); i++) {
        if (!keep[i]) continue;
        for (size_t j = i + 1; j < dets.size(); j++) {
            if (keep[j] && dets[i].cls_id == dets[j].cls_id && iou(dets[i].box, dets[j].box) > thr)
                keep[j] = false;
        }
    }
    std::vector<Detection> out;
    out.reserve(dets.size());
    for (size_t i = 0; i < dets.size(); i++) if (keep[i]) out.push_back(dets[i]);
    dets = std::move(out);
}

void YoloV8Detector::draw(cv::Mat& bgr, const std::vector<Detection>& results) {
    for (const auto& d : results) {
        cv::rectangle(bgr, d.box, cv::Scalar(0, 0, 255), 2);
        std::string txt = class_name(d.cls_id) + " " + cv::format("%.2f", d.score);
        int baseline = 0;
        cv::Size ts = cv::getTextSize(txt, cv::FONT_HERSHEY_SIMPLEX, 0.5, 1, &baseline);
        cv::rectangle(bgr, cv::Point(d.box.x, d.box.y - ts.height - 4),
                      cv::Point(d.box.x + ts.width, d.box.y), cv::Scalar(0, 0, 255), -1);
        cv::putText(bgr, txt, cv::Point(d.box.x, d.box.y - 2),
                    cv::FONT_HERSHEY_SIMPLEX, 0.5, cv::Scalar(255, 255, 255), 1);
    }
}
