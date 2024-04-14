package main

import (
	"context"
	"errors"
	"slices"
	"sync"
)

var NoAvailableNodeErr = errors.New("No Available Node")

type Storage interface {
	// get all node, no sort
	GetNodes(ctx context.Context) ([]Node, error)
	// get room node,sort by time,the first master node
	GetRoomNodes(ctx context.Context, room string) ([]Node, error)
}

func GetMaxIdlenessNode(ctx context.Context, nodes []Node, checkPub bool) (*Node, error) {
	if len(nodes) == 0 {
		return nil, NoAvailableNodeErr
	}
	nodes = slices.Clone(nodes)
	nodeMetricsMap := GetNodesMetrics(nodes)
	nodes = GetAvailableNodes(nodes, nodeMetricsMap, checkPub)
	if len(nodes) == 0 {
		return nil, NoAvailableNodeErr
	}
	NodeSort(nodes, nodeMetricsMap)
	return &nodes[len(nodes)-1], nil
}

func GetNodesMetrics(nodes []Node) map[string]*NodeMetrics {
	nodeMetricsMap := make(map[string]*NodeMetrics)
	var lock sync.Mutex
	var waitGroup sync.WaitGroup
	for _, node := range nodes {
		waitGroup.Add(1)
		go func(node Node) {
			defer waitGroup.Done()
			metrics, err := node.GetMetrics()
			if err != nil {
				return
			}
			lock.Lock()
			defer lock.Unlock()
			nodeMetricsMap[node.Addr] = metrics
		}(node)
	}
	waitGroup.Wait()
	return nodeMetricsMap
}

func GetAvailableNodes(nodes []Node, nodeMetricsMap map[string]*NodeMetrics, checkPub bool) []Node {
	nodes = slices.DeleteFunc(nodes, func(node Node) bool {
		metrics := nodeMetricsMap[node.Addr]
		metadata := node.Metadata
		return metrics == nil || (checkPub && metrics.Room >= metadata.PubMax) || metrics.Subscribe >= metadata.SubMax
	})
	return nodes
}

func NodeSort(nodes []Node, nodeMetricsMap map[string]*NodeMetrics) {
	slices.SortFunc(nodes, func(a, b Node) int {
		aNoneAvailableSub := a.Metadata.SubMax - nodeMetricsMap[a.Addr].Subscribe
		bNoneAvailableSub := b.Metadata.SubMax - nodeMetricsMap[b.Addr].Subscribe
		if aNoneAvailableSub < bNoneAvailableSub {
			return -1
		}
		if aNoneAvailableSub > bNoneAvailableSub {
			return 1
		}
		return 0
	})
}
