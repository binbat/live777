#!/bin/bash
# PX4 v1.17 SITL + Gazebo Jetty on Ubuntu 26.04 (image px4-jetty:resolute):
#  - reconfigure + build (picks up gstreamer, builds GstCameraSystem)
#  - gz server: forest world, PX4 server.config (Sensors+GstCameraSystem),
#    --headless-rendering (EGL on /dev/dri) so camera sensors render headless
#  - PX4 spawns x500_mono_cam; GstCameraSystem streams the onboard camera as
#    H.264/RTP to 127.0.0.1:5600 -> live777 SDP source (stream "fpv")
#  - Gazebo GUI as a Wayland client of the host live777 compositor ("demo")
set -x
exec docker run --rm -t \
  --name px4-jetty \
  --network host \
  --device /dev/dri \
  -v /tmp/px4-1.17:/src/PX4-Autopilot \
  -v /run/user/1000:/run/user/1000 \
  -e XDG_RUNTIME_DIR=/run/user/1000 \
  -e WAYLAND_DISPLAY=live777-demo \
  -e QT_QPA_PLATFORM=wayland \
  -e DEBIAN_FRONTEND=noninteractive \
  px4-jetty:resolute \
  bash -c "
    set -e
    git config --global --add safe.directory '*'
    cd /src/PX4-Autopilot
    echo '=== reconfigure + build (ninja, incremental) ==='
    cmake -S . -B build/px4_sitl_default >/dev/null
    ninja -C build/px4_sitl_default

    . build/px4_sitl_default/rootfs/gz_env.sh
    echo '=== starting gz server: forest, headless-rendering, PX4 plugins ==='
    nohup gz sim -r -s --headless-rendering \"\$PX4_GZ_WORLDS/forest.sdf\" >/tmp/gzserver.log 2>&1 &
    until gz topic -l 2>/dev/null | grep -q '/world/forest/clock'; do sleep 1; done
    echo '=== forest world up, starting px4 ==='

    cd build/px4_sitl_default
    PX4_SIM_MODEL=gz_x500_mono_cam GZ_IP=127.0.0.1 ./bin/px4 &
    PX4_PID=\$!

    echo '=== waiting for the camera image topic ==='
    for i in \$(seq 1 90); do
      gz topic -l 2>/dev/null | grep -q 'sensor/.*/image' && break
      sleep 2
    done
    gz topic -l 2>/dev/null | grep -E 'image' | head -3 || true

    echo '=== starting gz gui on wayland ==='
    nohup gz sim -g >/tmp/gzgui.log 2>&1 &

    wait \$PX4_PID
  "
