cmake_minimum_required(VERSION 3.10)
project(yolov8_detector_cpp CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_BUILD_TYPE Release)

# ---------- OpenCV ----------
find_package(OpenCV REQUIRED)

# ---------- RKNN ----------
# 默认使用板端系统路径；如不一致可在 cmake 时覆盖:
#   -DRKNN_INC=/path/include -DRKNN_LIB=/path/librknnrt.so
set(RKNN_INC "/usr/include" CACHE PATH "rknn api include dir")
set(RKNN_LIB "/usr/lib/librknnrt.so" CACHE FILEPATH "rknn runtime library")

include_directories(
    ${CMAKE_SOURCE_DIR}/include
    ${RKNN_INC}
    ${OpenCV_INCLUDE_DIRS}
)

add_executable(yolov8_detector
    src/main.cc
    src/yolov8_detector.cc
)

target_link_libraries(yolov8_detector
    ${OpenCV_LIBS}
    ${RKNN_LIB}
)
