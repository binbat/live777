cmake_minimum_required(VERSION 3.15)
project(libcamera-bridge VERSION 1.0.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# --- Explicit backend / capture / encoder options ---
# These are set by liveion/build.rs via cmake_config.define().
# They MUST NOT be inferred from TARGET or system architecture.

option(ENABLE_BACKEND_PI "Enable Raspberry Pi backend" OFF)
option(ENABLE_BACKEND_RDK_X5 "Enable RDK X5 backend" OFF)

option(ENABLE_CAPTURE_LIBCAMERA "Enable libcamera capture backend" OFF)
option(ENABLE_CAPTURE_V4L2 "Enable V4L2 capture backend" OFF)

option(ENABLE_ENCODER_V4L2_M2M "Enable V4L2 M2M encoder backend" OFF)
option(ENABLE_ENCODER_RDK_X5 "Enable RDK X5 encoder backend" OFF)

# --- Collect sources based on enabled options ---

set(SOURCES "")

if(ENABLE_CAPTURE_LIBCAMERA)
    message(STATUS "cambridge: libcamera capture enabled")
    list(APPEND SOURCES camera.cpp)
endif()

# V4L2 capture: RDK and generic paths are mutually exclusive — they define
# the same V4L2CaptureImpl class and C ABI symbols.
if(ENABLE_CAPTURE_V4L2)
  if(ENABLE_BACKEND_RDK_X5)
    list(APPEND SOURCES v4l2_capture_rdk.cpp)
    message(STATUS "cambridge: RDK X5 V4L2 capture enabled")
  else()
    list(APPEND SOURCES v4l2_capture.cpp)
    message(STATUS "cambridge: generic V4L2 capture enabled")
  endif()
endif()

if(ENABLE_ENCODER_V4L2_M2M)
    message(STATUS "cambridge: V4L2 M2M encoder enabled")
    list(APPEND SOURCES encoder.cpp)
endif()

if(ENABLE_ENCODER_RDK_X5)
    message(STATUS "cambridge: RDK X5 encoder enabled")
    list(APPEND SOURCES encoder_rdk.cpp)
endif()

if(ENABLE_BACKEND_PI)
    message(STATUS "cambridge: Raspberry Pi backend enabled")
endif()

if(ENABLE_BACKEND_RDK_X5)
    message(STATUS "cambridge: RDK X5 backend enabled")
    add_definitions(-DPLATFORM_RDK)
endif()

# SourcePipeline (platform-neutral, always built)
list(APPEND SOURCES src/pipeline/source_pipeline.cpp src/pipeline/backend_factory.cpp)

# --- Static library ---
add_library(cambridge STATIC ${SOURCES})

# Feature compile definitions for conditional compilation in backend_factory.cpp
if(ENABLE_CAPTURE_LIBCAMERA)
    target_compile_definitions(cambridge PRIVATE ENABLE_CAPTURE_LIBCAMERA=1)
endif()
if(ENABLE_CAPTURE_V4L2)
    target_compile_definitions(cambridge PRIVATE ENABLE_CAPTURE_V4L2=1)
endif()
if(ENABLE_BACKEND_RDK_X5)
    target_compile_definitions(cambridge PRIVATE ENABLE_BACKEND_RDK_X5=1)
endif()
if(ENABLE_ENCODER_V4L2_M2M)
    target_compile_definitions(cambridge PRIVATE ENABLE_ENCODER_V4L2_M2M=1)
endif()
if(ENABLE_ENCODER_RDK_X5)
    target_compile_definitions(cambridge PRIVATE ENABLE_ENCODER_RDK_X5=1)
endif()

target_include_directories(cambridge PRIVATE ${CMAKE_CURRENT_SOURCE_DIR})

# Platform-specific include dirs
if(ENABLE_CAPTURE_LIBCAMERA)
    find_package(PkgConfig REQUIRED)
    pkg_check_modules(LIBCAMERA REQUIRED libcamera)
    target_include_directories(cambridge PRIVATE ${LIBCAMERA_INCLUDE_DIRS})
endif()

if(ENABLE_BACKEND_RDK_X5)
    if(DEFINED RDK_SYSROOT)
        target_include_directories(cambridge PRIVATE ${RDK_SYSROOT}/usr/hobot/include)
    else()
        target_include_directories(cambridge PRIVATE /usr/hobot/include)
    endif()
endif()

# --- Link libraries ---
target_link_libraries(cambridge PRIVATE pthread)

if(ENABLE_CAPTURE_LIBCAMERA)
    target_link_libraries(cambridge PRIVATE ${LIBCAMERA_LIBRARIES})
endif()

if(ENABLE_BACKEND_RDK_X5)
    if(DEFINED RDK_SYSROOT)
        target_link_directories(cambridge PRIVATE ${RDK_SYSROOT}/usr/hobot/lib)
    else()
        target_link_directories(cambridge PRIVATE /usr/hobot/lib)
    endif()
    target_link_libraries(cambridge PRIVATE multimedia hbmem vpf)
endif()

# --- Compile options ---
target_compile_options(cambridge PRIVATE -Wall -Wextra)

if(ENABLE_CAPTURE_LIBCAMERA)
    target_compile_options(cambridge PRIVATE ${LIBCAMERA_CFLAGS_OTHER})
endif()

# --- Install ---
install(TARGETS cambridge
    ARCHIVE DESTINATION lib
    LIBRARY DESTINATION lib
)
