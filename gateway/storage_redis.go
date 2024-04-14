package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"

	"github.com/redis/go-redis/v9"
)

const NodesRegistryKey = "live777:nodes"

const NodeRegistryKey = "live777:node"

const RoomRegistryKey = "live777:room"

type RedisStandaloneStorage struct {
	client *redis.Client
}

func NewRedisStandaloneStorage(addr string) (*RedisStandaloneStorage, error) {
	url, err := redis.ParseURL(addr)
	if err != nil {
		return nil, err
	}
	client := redis.NewClient(url)
	// check conn
	cmd := client.Get(context.Background(), "hello world")
	if cmd.Err() != nil && !errors.Is(cmd.Err(), redis.Nil) {
		return nil, fmt.Errorf("redis conn cmd error : %v", cmd.Err())
	}
	return &RedisStandaloneStorage{
		client: client,
	}, nil
}

func (r *RedisStandaloneStorage) GetNodes(ctx context.Context) ([]Node, error) {
	getNodesCmd := r.client.SMembers(ctx, NodesRegistryKey)
	nodes, delNodes, err := r.getFinalNodes(ctx, getNodesCmd)
	if err != nil {
		return nil, err
	}
	r.client.SRem(ctx, NodesRegistryKey, delNodes...)
	return nodes, nil
}

func (r *RedisStandaloneStorage) GetRoomNodes(ctx context.Context, room string) ([]Node, error) {
	getNodesCmd := r.client.ZRange(ctx, fmt.Sprintf("%s:%s", RoomRegistryKey, room), 0, -1)
	nodes, delNodes, err := r.getFinalNodes(ctx, getNodesCmd)
	if err != nil {
		return nil, err
	}
	r.client.ZRem(ctx, NodesRegistryKey, delNodes...)
	finalNodes := make([]Node, 0)
	for _, node := range nodes {
		info, _ := node.GetRoomInfo(room)
		if info == nil {
			r.client.ZRem(ctx, fmt.Sprintf("%s:%s", RoomRegistryKey, room), node.Addr)
		} else {
			finalNodes = append(finalNodes, node)
		}
	}

	return finalNodes, nil
}

func (r *RedisStandaloneStorage) getFinalNodes(ctx context.Context, getNodesCmd *redis.StringSliceCmd) ([]Node, []interface{}, error) {
	if getNodesCmd.Err() != nil {
		if !errors.Is(getNodesCmd.Err(), redis.Nil) {
			return nil, nil, fmt.Errorf("redis conn getNodesCmd error : %v", getNodesCmd.Err())
		}
		return make([]Node, 0), nil, nil
	}
	nodes := getNodesCmd.Val()
	if len(nodes) == 0 {
		return nil, nil, nil
	}
	nodeKeys := make([]string, len(nodes))
	for i, node := range nodes {
		nodeKeys[i] = fmt.Sprintf("%s:%s", NodeRegistryKey, node)
	}
	mgetCmd := r.client.MGet(ctx, nodeKeys...)
	if mgetCmd.Err() != nil {
		return nil, nil, mgetCmd.Err()
	}
	nodeValues := mgetCmd.Val()
	resNodes := make([]Node, 0)
	delNodes := make([]interface{}, 0)
	for index, node := range nodes {
		nodeValue := nodeValues[index]
		if nodeValue == nil {
			delNodes = append(delNodes, node)
		} else {
			metaData := NodeMetaData{}
			json.Unmarshal([]byte(fmt.Sprintf("%s", nodeValue)), &metaData)
			resNodes = append(resNodes, Node{
				Addr:     node,
				Metadata: metaData,
			})
		}
	}
	return resNodes, delNodes, nil
}
