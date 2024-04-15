package main

import (
	"encoding/json"
	"fmt"
	"testing"
)

func TestNode_GetMetrics(t *testing.T) {
	node := Node{Addr: "127.0.0.1:7777"}
	metrics, err := node.GetMetrics()
	if err != nil {
		panic(err)
	}
	data, _ := json.Marshal(metrics)
	fmt.Printf("%s", data)
}

func TestNode_GetStreamInfos(t *testing.T) {
	node := Node{Addr: "127.0.0.1:7777"}
	infos, err := node.GetStreamInfos()
	if err != nil {
		panic(err)
	}
	data, _ := json.Marshal(infos)
	fmt.Printf("%s", data)
}

func TestNode_GetStreamInfo(t *testing.T) {
	node := Node{Addr: "127.0.0.1:7777"}
	info, err := node.GetStreamInfo("7777")
	if err != nil {
		panic(err)
	}
	data, _ := json.Marshal(info)
	fmt.Printf("%s", data)
}

func TestReforwardInfo_ParseNodeAndStream(t *testing.T) {
	node, stream := ReforwardInfo{TargetUrl: "http://127.0.0.1:7777/whip/7777"}.ParseNodeAndStream()
	fmt.Printf("node : %s, stream : %s\n", node, stream)
}

func TestNode_Reforward(t *testing.T) {
	node := Node{Addr: "127.0.0.1:7777"}
	targetNode := Node{Addr: "127.0.0.1:7777"}
	err := node.Reforward(targetNode, "7777", "8888")
	if err != nil {
		panic(err)
	}
}
