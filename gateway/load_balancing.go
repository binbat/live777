package main

import (
	"context"
	"errors"
	"math/rand"
	"sync/atomic"
)

type LoadBalancing interface {
	Next(context.Context, Storage) (*Node, error)
}

type RandomLoadBalancing struct{}

func (r *RandomLoadBalancing) Next(ctx context.Context, s Storage) (*Node, error) {
	nodes, err := storage.GetAllNode(ctx)
	if err != nil {
		return nil, err
	}
	if len(nodes) == 0 {
		return nil, errors.New("there are no nodes to provide services")
	}
	index := rand.Intn(len(nodes))
	return &nodes[index], nil
}

type LocalPollingLoadBalancing struct {
	offset uint64
}

func (l *LocalPollingLoadBalancing) Next(ctx context.Context, s Storage) (*Node, error) {
	nodes, err := storage.GetAllNode(ctx)
	if err != nil {
		return nil, err
	}
	if len(nodes) == 0 {
		return nil, errors.New("there are no nodes to provide services")
	}
	offset := atomic.AddUint64(&l.offset, 1)
	return &nodes[int(offset)%len(nodes)], nil
}
