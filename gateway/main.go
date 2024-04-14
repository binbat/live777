package main

import (
	"context"
	"embed"
	"errors"
	"flag"
	"github.com/gorilla/mux"
	"io/fs"
	"log/slog"
	"net/http"
	"net/http/httputil"
	"os"
	"sync"
	"time"
)

var config *Config

var storage Storage

//go:embed assets
var assets embed.FS

func init() {
	configPath := flag.String("config", "config.toml", "load config file")
	flag.Parse()
	config = ParseConfig(*configPath)
	var err error
	var level slog.Level
	err = level.UnmarshalText([]byte(config.Level))
	if err != nil {
		panic(err)
	}
	slog.SetDefault(slog.New(slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{Level: level})))
	slog.Info("init", "config", config)
	switch config.Model {
	case "RedisStandalone":
		storage, err = NewRedisStandaloneStorage(config.Addr)
		if err != nil {
			panic(err)
		}
	}
	if storage == nil {
		panic("storage is null,please check config")
	}

}

func main() {
	assets, err := fs.Sub(assets, "assets")
	if err != nil {
		panic(err)
	}
	r := mux.NewRouter()
	r.HandleFunc("/whip/{room}", whipHandler)
	r.HandleFunc("/whep/{room}", whepHandler)
	r.HandleFunc("/resource/{room}/{session}", resourceHandler)
	r.HandleFunc("/resource/{room}/{session}/layer", resourceHandler)
	r.PathPrefix("/").Handler(http.StripPrefix("/", http.FileServer(http.FS(assets))))
	r.Use(loggingMiddleware)
	r.Use(mux.CORSMethodMiddleware(r))
	go checkReforwardTick(context.Background())
	slog.Info("Http ListenAndServe Start", "ListenAddr", config.ListenAddr)
	panic(http.ListenAndServe(config.ListenAddr, r))
}

func whipHandler(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	room := extractRequestRoom(r)
	nodes, err := storage.GetRoomNodes(ctx, room)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	var targetNode *Node
	if len(nodes) != 0 {
		targetNode = &nodes[0]
	} else {
		nodes, err := storage.GetNodes(ctx)
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		node, err := GetMaxIdlenessNode(ctx, nodes, true)
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		targetNode = node
	}
	doProxy(w, r, *targetNode)
}

func whepHandler(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	room := extractRequestRoom(r)
	roomNodes, err := storage.GetRoomNodes(ctx, room)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	if len(roomNodes) == 0 {
		http.Error(w, "the room does not exist", http.StatusNotFound)
		return
	}
	var targetNode *Node
	node, err := GetMaxIdlenessNode(ctx, roomNodes, false)
	if err == nil {
		targetNode = node
	} else {
		if errors.Is(err, NoAvailableNodeErr) {
			targetNode, err = whepGetReforwardNode(roomNodes, ctx, room)
			if err != nil {
				http.Error(w, err.Error(), http.StatusInternalServerError)
				return
			}
		} else {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
	}
	doProxy(w, r, *targetNode)
}

func whepGetReforwardNode(roomNodes []Node, ctx context.Context, room string) (*Node, error) {
	var reforwardNode *Node
	for _, node := range roomNodes {
		if !node.Metadata.ReforwardCascade {
			reforwardNode = &node
			break
		}
	}
	if reforwardNode == nil {
		reforwardNode = &roomNodes[len(roomNodes)-1]
	}
	nodes, err := storage.GetNodes(ctx)
	if err != nil {
		return nil, err
	}
	idlenessNode, err := GetMaxIdlenessNode(ctx, nodes, true)
	if err != nil {
		return nil, err
	}
	err = reforwardNode.Reforward(*idlenessNode, room, room)
	slog.Info("reforward", "room", room, "reforwardNode", reforwardNode, "targetNode", idlenessNode, "error", err)
	if err != nil {
		return nil, err
	}
	for i := 0; i < config.ReforwardCheckFrequency; i++ {
		time.Sleep(time.Millisecond * 50)
		info, _ := idlenessNode.GetRoomInfo(room)
		if info != nil && info.PublishSessionInfo != nil && info.PublishSessionInfo.ConnectState == RTCPeerConnectionStateConnected {
			break
		}
	}
	return idlenessNode, nil
}

func resourceHandler(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	vars := mux.Vars(r)
	room := vars["room"]
	session := vars["session"]
	nodes, err := storage.GetRoomNodes(ctx, room)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	for _, node := range nodes {
		info, _ := node.GetRoomInfo(room)
		if info == nil {
			continue
		}
		if info.PublishSessionInfo != nil && info.PublishSessionInfo.Id == session {
			doProxy(w, r, node)
			return
		}
		for _, subscribeSessionInfo := range info.SubscribeSessionInfos {
			if subscribeSessionInfo.Id == session {
				doProxy(w, r, node)
				return
			}
		}
	}
	http.Error(w, "the session does not exist", http.StatusNotFound)
}

func extractRequestRoom(r *http.Request) string {
	vars := mux.Vars(r)
	return vars["room"]
}

func doProxy(w http.ResponseWriter, r *http.Request, node Node) {
	slog.Info("http server request proxy", "URI", r.RequestURI, "node", node)
	proxy := httputil.ReverseProxy{
		Transport: &loggingTransport{
			operation: "PROXY",
			transport: http.DefaultTransport,
		},
		Director: func(req *http.Request) {
			req.URL.Scheme = "http"
			req.URL.Host = node.Addr
			req.Host = node.Addr
			authorization := node.Metadata.Authorization
			if authorization != nil {
				req.Header.Set("Authorization", *authorization)
			}
		},
	}
	proxy.ServeHTTP(w, r)
}

func checkReforwardTick(ctx context.Context) {
	ticker := time.NewTicker(time.Millisecond * time.Duration(config.CheckReforwardTickTime))
	for {
		select {
		case <-ticker.C:
			doCheckReforward(ctx)
		case <-ctx.Done():
			return
		}
	}
}

func doCheckReforward(ctx context.Context) {
	nodes, err := storage.GetNodes(ctx)
	if err != nil {
		return
	}
	nodeMap := make(map[string]Node)
	for _, node := range nodes {
		nodeMap[node.Addr] = node
	}
	nodesRoomInfos := getNodesRoomInfos(nodes)
	for _, node := range nodes {
		roomInfos := nodesRoomInfos[node.Addr]
		for _, roomInfo := range roomInfos {
			for _, subscribeSessionInfo := range roomInfo.SubscribeSessionInfos {
				if subscribeSessionInfo.Reforward != nil {
					reforwardNodeAddr, reforwardNodeRoom := subscribeSessionInfo.Reforward.ParseNodeAndRoom()
					reforwardNode, ok := nodeMap[reforwardNodeAddr]
					if !ok {
						continue
					}
					reforwardNodeRoomInfo, err := reforwardNode.GetRoomInfo(reforwardNodeRoom)
					if err != nil {
						continue
					}
					if reforwardNodeRoomInfo.SubscribeLeaveTime != 0 && time.Now().UnixMilli() >= int64(reforwardNodeRoomInfo.SubscribeLeaveTime)+int64(node.Metadata.ReforwardMaximumIdleTime) {
						slog.Info("reforward idle for long periods of time",
							"node", node,
							"room", roomInfo.Id,
							"session", subscribeSessionInfo.Id,
							"reforwardNode", reforwardNode,
							"reforwardNodeRoomInfo", reforwardNodeRoomInfo)
						_ = node.ResourceDelete(roomInfo.Id, subscribeSessionInfo.Id)
					}
				}
			}
		}
	}
}

func getNodesRoomInfos(nodes []Node) map[string][]RoomInfo {
	nodeRoomInfosMap := make(map[string][]RoomInfo)
	var lock sync.Mutex
	var waitGroup sync.WaitGroup
	for _, node := range nodes {
		waitGroup.Add(1)
		go func(node Node) {
			defer waitGroup.Done()
			infos, err := node.GetRoomInfos()
			if err == nil {
				lock.Lock()
				defer lock.Unlock()
				nodeRoomInfosMap[node.Addr] = infos
			}
		}(node)
	}
	waitGroup.Wait()
	return nodeRoomInfosMap
}
