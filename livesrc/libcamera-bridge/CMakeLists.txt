cmake_minimum_required(VERSION 3.15)
project(libcamera-bridge VERSION 1.0.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# Find libcamera
find_package(PkgConfig REQUIRED)
pkg_check_modules(LIBCAMERA REQUIRED libcamera)

# Source files
set(SOURCES
    main.cpp
    camera.cpp
    encoder.cpp
)

# Executable
add_executable(libcamera-bridge ${SOURCES})

# Include directories
target_include_directories(libcamera-bridge PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}
    ${LIBCAMERA_INCLUDE_DIRS}
)

# Link libraries
target_link_libraries(libcamera-bridge PRIVATE
    ${LIBCAMERA_LIBRARIES}
    pthread
)

# Compiler options
target_compile_options(libcamera-bridge PRIVATE
    -Wall
    -Wextra
    -Werror
    ${LIBCAMERA_CFLAGS_OTHER}
)

# Install
install(TARGETS libcamera-bridge
    RUNTIME DESTINATION bin
)
