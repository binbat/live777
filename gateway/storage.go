package main

import "context"

type Storage interface {
	GetAllNode(ctx context.Context) ([]Node, error)
	GetRoomOwnership(ctx context.Context, room string) (*Node, error)
}
