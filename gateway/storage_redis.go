package main

import (
	"context"
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

func (r *RedisStandaloneStorage) GetAllNode(ctx context.Context) ([]Node, error) {
	getNodesCmd := r.client.SInter(ctx, NodesRegistryKey)
	if getNodesCmd.Err() != nil {
		if !errors.Is(getNodesCmd.Err(), redis.Nil) {
			return nil, fmt.Errorf("redis conn getNodesCmd error : %v", getNodesCmd.Err())
		}
		return make([]Node, 0), nil
	}
	nodes := getNodesCmd.Val()
	nodeKeys := make([]string, len(nodes))
	for i, node := range nodes {
		nodeKeys[i] = fmt.Sprintf("%s:%s", NodeRegistryKey, node)
	}
	mgetCmd := r.client.MGet(ctx, nodeKeys...)
	if mgetCmd.Err() != nil {
		return nil, mgetCmd.Err()
	}
	nodeValues := mgetCmd.Val()
	resNodes := make([]Node, 0)
	delNodes := make([]interface{}, 0)
	for index, node := range nodes {
		nodeValue := nodeValues[index]
		if nodeValue == nil {
			delNodes = append(delNodes, node)
		} else {
			resNodes = append(resNodes, Node{
				Addr:     node,
				Metadata: fmt.Sprintf("%s", nodeValue),
			})
		}
	}
	r.client.SRem(ctx, NodesRegistryKey, delNodes...)
	return resNodes, nil
}

func (r *RedisStandaloneStorage) GetRoomOwnership(ctx context.Context, room string) (*Node, error) {
	getRoomCmd := r.client.Get(ctx, fmt.Sprintf("%s:%s", RoomRegistryKey, room))
	if getRoomCmd.Err() != nil {
		if errors.Is(getRoomCmd.Err(), redis.Nil) {
			return nil, nil
		}
		return nil, getRoomCmd.Err()
	}
	roomNode := getRoomCmd.Val()
	getNodeCmd := r.client.Get(ctx, fmt.Sprintf("%s:%s", NodeRegistryKey, roomNode))
	if getNodeCmd.Err() != nil {
		return nil, getNodeCmd.Err()
	}
	return &Node{
		roomNode, getNodeCmd.Val(),
	}, nil
}
