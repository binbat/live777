cmake_minimum_required(VERSION 3.15)
project(libcamera-bridge VERSION 1.0.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# Find libcamera
find_package(PkgConfig REQUIRED)
pkg_check_modules(LIBCAMERA REQUIRED libcamera)

# Source files
set(SOURCES
    camera.cpp
    encoder.cpp
    bridge_ffi.cpp
)

# Link directories (help CMake find the sysroot libraries if needed)
link_directories(${LIBCAMERA_LIBRARY_DIRS})

# Static Library for Rust FFI
add_library(cambridge STATIC ${SOURCES})

target_include_directories(cambridge PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}
    ${LIBCAMERA_INCLUDE_DIRS}
)

target_link_libraries(cambridge PRIVATE
    ${LIBCAMERA_LIBRARIES}
    pthread
)

target_compile_options(cambridge PRIVATE
    -Wall
    -Wextra
    -Werror
    ${LIBCAMERA_CFLAGS_OTHER}
)

# Install rules for the static library
install(TARGETS cambridge
    ARCHIVE DESTINATION lib
    LIBRARY DESTINATION lib
)
