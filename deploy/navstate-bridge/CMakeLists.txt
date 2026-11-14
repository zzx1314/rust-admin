cmake_minimum_required(VERSION 3.8)
project(navstate_bridge)

if(NOT CMAKE_CXX_STANDARD)
  set(CMAKE_CXX_STANDARD 17)
  set(CMAKE_CXX_STANDARD_REQUIRED ON)
endif()

if(CMAKE_COMPILER_IS_GNUCXX OR CMAKE_CXX_COMPILER_ID MATCHES "Clang")
  add_compile_options(-Wall -Wextra -Wpedantic)
endif()

find_package(ament_cmake REQUIRED)
find_package(rclcpp REQUIRED)

add_executable(navstate_bridge src/navstate_bridge.cpp)
target_link_libraries(navstate_bridge rclcpp::rclcpp)
install(TARGETS navstate_bridge DESTINATION lib/${PROJECT_NAME})

ament_package()
