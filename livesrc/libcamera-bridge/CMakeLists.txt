cmake_minimum_required(VERSION 3.15)
project(libcamera-bridge VERSION 1.0.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

option(PLATFORM_RDK "Build for Horizon RDK X5" OFF)

if(PLATFORM_RDK)
    message(STATUS "Configuring for Horizon RDK X5 Platform")
    set(SOURCES
        encoder_rdk.cpp
        v4l2_capture_rdk.cpp
        bridge_v4l2_rdk_ffi.cpp
    )
    add_definitions(-DPLATFORM_RDK)
    
    # RDK specific libraries (Sunrise 5)
    link_directories(/usr/hobot/lib)
    set(HB_LIBS multimedia hbmem vpf)
else()
    message(STATUS "Configuring for Raspberry Pi Platform")
    find_package(PkgConfig REQUIRED)
    pkg_check_modules(LIBCAMERA REQUIRED libcamera)
    
    set(SOURCES
        camera.cpp
        encoder.cpp
        bridge_ffi.cpp
        v4l2_capture.cpp
        bridge_v4l2_ffi.cpp
    )
endif()

# Static Library for Rust FFI
add_library(cambridge STATIC ${SOURCES})

target_include_directories(cambridge PRIVATE
     ${CMAKE_CURRENT_SOURCE_DIR}
     ${LIBCAMERA_INCLUDE_DIRS}
     /usr/hobot/include
 )

if(PLATFORM_RDK)
    target_link_libraries(cambridge PRIVATE
        ${HB_LIBS}
        pthread
    )
else()
    target_link_libraries(cambridge PRIVATE
        ${LIBCAMERA_LIBRARIES}
        pthread
    )
endif()

target_compile_options(cambridge PRIVATE
    -Wall
    -Wextra
    ${LIBCAMERA_CFLAGS_OTHER}
)

# Install rules for the static library
install(TARGETS cambridge
    ARCHIVE DESTINATION lib
    LIBRARY DESTINATION lib
)
