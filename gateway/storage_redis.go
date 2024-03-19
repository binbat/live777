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
	val := getNodesCmd.Val()
	nodes := make([]Node, 0)
	for _, node := range val {
		getNodeCmd := r.client.Get(ctx, fmt.Sprintf("%s:%s", NodeRegistryKey, node))
		if getNodeCmd.Err() != nil {
			if errors.Is(getNodeCmd.Err(), redis.Nil) {
				r.client.SRem(ctx, NodesRegistryKey, node)
			}
			continue
		}
		nodeMetadata := getNodeCmd.Val()
		nodes = append(nodes, Node{node, nodeMetadata})
	}
	return nodes, nil
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
