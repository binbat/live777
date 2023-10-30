#!/bin/bash

# -----
# @Name: multirun
# @Description: Multiple commands execution
# @Example ./multirun.sh 'python3 -m http.server 8001' 'python3 -m http.server 8002'
# @Author: Metal A-wing
# @Email: 1@233.email
# @Create: 2019-10-18T12:10:00+08:00
# @Update: 2023-07-14T17:00:00+08:00
# -----

ARGC=$#

if [[ ${ARGC} == 0 ]]; then
  echo -e "\033[44;37;5m INPUT \033[0m ./multirun.sh 'python3 -m http.server 8001' 'python3 -m http.server 8002' '<...>'"
  exit
fi

echo -e "\033[44;37;5m RUNING \033[0m  ${ARGC} COMMANDS"

for id in $(seq 1 $ARGC); do
  $(eval echo '$'{$id}) &
  PID[id]=$!

  echo "RUN ${id} pid ${PID[id]} : $(eval echo '$'{$id})"
done

# trap ctrl-c and call ctrl_c()
trap ctrl_c INT

function ctrl_c() {
  echo "** Trapped CTRL-C"
  echo -e "\033[41;37;5m STOP \033[0m  ${ARGC} COMMANDS"

  for id in $(seq 1 $ARGC); do
    echo "STOP ${id} pid ${PID[id]}"
    kill ${PID[id]}
  done

  exit
}

while true; do
  sleep 0.1
done
