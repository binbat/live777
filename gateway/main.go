package main

import (
	"context"
	"embed"
	"errors"
	"flag"
	"io/fs"
	"log/slog"
	"net/http"
	"net/http/httputil"
	"os"
	"sync"
	"time"

	"github.com/gorilla/mux"
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
	r.HandleFunc("/whip/{stream}", whipHandler)
	r.HandleFunc("/whep/{stream}", whepHandler)
	r.HandleFunc("/resource/{stream}/{session}", resourceHandler)
	r.HandleFunc("/resource/{stream}/{session}/layer", resourceHandler)
	r.PathPrefix("/").Handler(http.StripPrefix("/", http.FileServer(http.FS(assets))))
	r.Use(loggingMiddleware)
	r.Use(mux.CORSMethodMiddleware(r))
	go checkReforwardTick(context.Background())
	slog.Info("Http ListenAndServe Start", "ListenAddr", config.ListenAddr)
	panic(http.ListenAndServe(config.ListenAddr, r))
}

func whipHandler(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	stream := extractRequestStream(r)
	nodes, err := storage.GetStreamNodes(ctx, stream)
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
	stream := extractRequestStream(r)
	streamNodes, err := storage.GetStreamNodes(ctx, stream)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	if len(streamNodes) == 0 {
		http.Error(w, "the stream does not exist", http.StatusNotFound)
		return
	}
	var targetNode *Node
	node, err := GetMaxIdlenessNode(ctx, streamNodes, false)
	if err == nil {
		targetNode = node
	} else {
		if errors.Is(err, ErrNoAvailableNode) {
			targetNode, err = whepGetReforwardNode(streamNodes, ctx, stream)
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

func whepGetReforwardNode(streamNodes []Node, ctx context.Context, stream string) (*Node, error) {
	var reforwardNode *Node
	for _, node := range streamNodes {
		if !node.Metadata.ReforwardCascade {
			reforwardNode = &node
			break
		}
	}
	if reforwardNode == nil {
		reforwardNode = &streamNodes[len(streamNodes)-1]
	}
	nodes, err := storage.GetNodes(ctx)
	if err != nil {
		return nil, err
	}
	targetNode, err := GetMaxIdlenessNode(ctx, nodes, true)
	if err != nil {
		return nil, err
	}
	err = reforwardNode.Reforward(*targetNode, stream, stream)
	slog.Info("reforward", "stream", stream, "reforwardNode", reforwardNode, "targetNode", targetNode, "error", err)
	if err != nil {
		return nil, err
	}
	for i := 0; i < config.ReforwardCheckFrequency; i++ {
		time.Sleep(time.Millisecond * 50)
		info, _ := targetNode.GetStreamInfo(stream)
		if info != nil && info.PublishSessionInfo != nil && info.PublishSessionInfo.ConnectState == RTCPeerConnectionStateConnected {
			break
		}
	}
	return targetNode, nil
}

func resourceHandler(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	vars := mux.Vars(r)
	stream := vars["stream"]
	session := vars["session"]
	nodes, err := storage.GetStreamNodes(ctx, stream)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	for _, node := range nodes {
		info, _ := node.GetStreamInfo(stream)
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

func extractRequestStream(r *http.Request) string {
	vars := mux.Vars(r)
	return vars["stream"]
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
	nodesStreamInfos := getNodesStreamInfos(nodes)
	for _, node := range nodes {
		streamInfos := nodesStreamInfos[node.Addr]
		for _, streamInfo := range streamInfos {
			for _, subscribeSessionInfo := range streamInfo.SubscribeSessionInfos {
				if subscribeSessionInfo.Reforward != nil {
					reforwardNodeAddr, reforwardNodestream := subscribeSessionInfo.Reforward.ParseNodeAndStream()
					reforwardNode, ok := nodeMap[reforwardNodeAddr]
					if !ok {
						continue
					}
					reforwardNodeStreamInfo, err := reforwardNode.GetStreamInfo(reforwardNodestream)
					if err != nil {
						continue
					}
					if reforwardNodeStreamInfo.SubscribeLeaveTime != 0 && time.Now().UnixMilli() >= int64(reforwardNodeStreamInfo.SubscribeLeaveTime)+int64(node.Metadata.ReforwardMaximumIdleTime) {
						slog.Info("reforward idle for long periods of time",
							"node", node,
							"stream", streamInfo.Id,
							"session", subscribeSessionInfo.Id,
							"reforwardNode", reforwardNode,
							"reforwardNodeStreamInfo", reforwardNodeStreamInfo)
						_ = node.ResourceDelete(streamInfo.Id, subscribeSessionInfo.Id)
					}
				}
			}
		}
	}
}

func getNodesStreamInfos(nodes []Node) map[string][]StreamInfo {
	nodeStreamInfosMap := make(map[string][]StreamInfo)
	var lock sync.Mutex
	var waitGroup sync.WaitGroup
	for _, node := range nodes {
		waitGroup.Add(1)
		go func(node Node) {
			defer waitGroup.Done()
			infos, err := node.GetStreamInfos()
			if err == nil {
				lock.Lock()
				defer lock.Unlock()
				nodeStreamInfosMap[node.Addr] = infos
			}
		}(node)
	}
	waitGroup.Wait()
	return nodeStreamInfosMap
}
