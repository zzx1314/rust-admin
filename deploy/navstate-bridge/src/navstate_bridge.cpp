// navstate_bridge —— /navigation_state 版本兼容桥（现场兜底，方案 3）
//
// 背景（2026-11-14 华致办公室现场）：
//   `.234`（算法机）的 robots_dog_msgs = 0.9.8，NavigationState.msg 末尾多一个
//   `uint8 current_floor`；`.1`（运控机）的 dog_task 是按 0.9.0（没有该字段）编译的。
//   CDR 是顺序编码：发布端多写 1 字节后，订阅端按旧布局读，读到数组长度之类的字段时
//   会拿到垃圾值 → `Fast CDR ... Not enough memory in the buffer stream` → 偶发
//   `std::bad_alloc` → dog_task SIGABRT（exit code -6）。
//
// 本节点的作用：把那条消息的序列化字节**末尾截掉 N 字节**（默认 1），再发到另一个话题；
// 让 dog_task 通过 remap 去订阅“截过”的版本，从而按 0.9.0 布局正常解析。
//
//          /waypoint_follower
//                 │  /navigation_state        （0.9.8 字节）
//                 ▼
//          [navstate_bridge]  ← 本节点（generic sub/pub，不需要两套 msg 定义）
//                 │  /navigation_state_v090   （0.9.0 字节）
//                 ▼
//              dog_task  （启动参数 -r navigation_state:=navigation_state_v090）
//
// 参数：
//   in_topic   （默认 /navigation_state）
//   out_topic  （默认 /navigation_state_v090）
//   type       （默认 robots_dog_msgs/msg/NavigationState；两端都用它，只是字节不同）
//   drop_tail  （默认 1，截掉末尾字节数；0 = 只做转发，用于“先看长度分布”的干跑）
//   publish    （默认 true；false = dry-run，只统计长度，不转发）
//   log_every  （默认 200，每 N 帧打一条统计日志）
//
// 编译（在有 ROS 2 Humble + 开发工具链的机器上，推荐 `.234`，产物可直接拷到 `.1`）：
//   mkdir -p ~/navstate_bridge_ws/src && cd ~/navstate_bridge_ws/src
//   cp -r /path/to/deploy/navstate-bridge navstate_bridge
//   cd .. && colcon build --packages-select navstate_bridge
//
// 运行：
//   source /opt/ros/humble/setup.bash
//   source ~/navstate_bridge_ws/install/setup.bash
//   ros2 run navstate_bridge navstate_bridge --ros-args \
//     -p in_topic:=/navigation_state -p out_topic:=/navigation_state_v090 -p drop_tail:=1

#include <cstring>
#include <memory>
#include <string>

#include <rclcpp/rclcpp.hpp>
#include <rclcpp/generic_publisher.hpp>
#include <rclcpp/generic_subscription.hpp>
#include <rclcpp/serialized_message.hpp>

int main(int argc, char ** argv)
{
  rclcpp::init(argc, argv);
  auto node = std::make_shared<rclcpp::Node>("navstate_bridge");

  const auto in_topic = node->declare_parameter<std::string>("in_topic", "/navigation_state");
  const auto out_topic = node->declare_parameter<std::string>("out_topic", "/navigation_state_v090");
  const auto topic_type = node->declare_parameter<std::string>(
    "type", "robots_dog_msgs/msg/NavigationState");
  const auto drop_tail = node->declare_parameter<int>("drop_tail", 1);
  const auto publish = node->declare_parameter<bool>("publish", true);
  const auto log_every = node->declare_parameter<int>("log_every", 200);
  // 发布端是 RELIABLE/VOLATILE/KEEP_LAST(10)；订阅端 dog_task 是 BEST_EFFORT —— RELIABLE 发布方
  // 与 BEST_EFFORT 订阅方兼容，所以这里统一用 reliable 就不会丢。
  const auto qos = rclcpp::QoS(rclcpp::KeepLast(10)).reliable();

  std::shared_ptr<rclcpp::GenericPublisher> publisher;
  if (publish) {
    publisher = node->create_generic_publisher(out_topic, topic_type, qos);
  }

  auto frames = std::make_shared<unsigned long long>(0);
  auto bytes_in = std::make_shared<unsigned long long>(0);
  auto forwarded = std::make_shared<unsigned long long>(0);
  auto skipped = std::make_shared<unsigned long long>(0);

  auto subscription = node->create_generic_subscription(
    in_topic, topic_type, qos,
    [publisher, drop_tail, publish, log_every, frames, bytes_in, forwarded, skipped, node,
     out_topic, in_topic](std::shared_ptr<rclcpp::SerializedMessage> msg) {
      const auto & raw = msg->get_rcl_serialized_message();
      ++(*frames);
      *bytes_in += raw.buffer_length;

      // CDR 流至少要能容纳 4 字节封装头；截完还要剩下有效内容
      if (drop_tail < 0 || raw.buffer_length <= static_cast<size_t>(drop_tail) + 4) {
        ++(*skipped);
        RCLCPP_WARN_THROTTLE(
          node->get_logger(), *node->get_clock(), 5000,
          "丢弃异常帧：buffer_length=%zu drop_tail=%d", raw.buffer_length, drop_tail);
        return;
      }

      if (!publish || !publisher) {
        if (log_every > 0 && (*frames % static_cast<unsigned long long>(log_every)) == 0) {
          RCLCPP_INFO(
            node->get_logger(), "[dry-run] %s 收到 %llu 帧，最近一帧 %zu 字节（将截为 %zu）",
            in_topic.c_str(), *frames, raw.buffer_length,
            raw.buffer_length - static_cast<size_t>(drop_tail));
        }
        return;
      }

      const size_t out_len = raw.buffer_length - static_cast<size_t>(drop_tail);
      rclcpp::SerializedMessage out(out_len);
      auto & out_raw = out.get_rcl_serialized_message();
      std::memcpy(out_raw.buffer, raw.buffer, out_len);
      out_raw.buffer_length = out_len;
      publisher->publish(out);
      ++(*forwarded);

      if (log_every > 0 && (*forwarded % static_cast<unsigned long long>(log_every)) == 0) {
        RCLCPP_INFO(
          node->get_logger(),
          "%s → %s 已转发 %llu/%llu 帧（累计 %llu 字节），最近 in=%zu out=%zu",
          in_topic.c_str(), out_topic.c_str(), *forwarded, *frames, *bytes_in,
          raw.buffer_length, out_len);
      }
    });

  RCLCPP_INFO(
    node->get_logger(),
    "navstate_bridge 启动：%s → %s（type=%s, drop_tail=%d, publish=%s）",
    in_topic.c_str(), out_topic.c_str(), topic_type.c_str(), drop_tail,
    publish ? "true" : "false");

  rclcpp::spin(node);
  rclcpp::shutdown();
  return 0;
}
