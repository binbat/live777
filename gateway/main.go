package main

import (
	"embed"
	"encoding/json"
	"flag"
	"fmt"
	"io/fs"
	"log"
	"net/http"
	"net/http/httputil"
	"net/url"
	"strconv"
	"time"

	"github.com/gorilla/mux"
)

var config *Config

var storage Storage

var loadBalancing LoadBalancing

//go:embed assets
var assets embed.FS

func init() {
	configPath := flag.String("config", "", "load config file")
	flag.Parse()

	config = ParseConfig(*configPath)
	switch config.Model {
	case "RedisStandalone":
		storage, _ = NewRedisStandaloneStorage(config.Addr)
	}
	data, _ := json.Marshal(config)
	log.Printf("config %s", data)
	switch config.LoadBalancing {
	case "random":
		loadBalancing = &RandomLoadBalancing{}
	case "localPolling":
		loadBalancing = &LocalPollingLoadBalancing{offset: 0}
	}
	if storage == nil {
		panic("storage is null,please check config")
	}
	if loadBalancing == nil {
		panic("loadBalancing is null,please check config")
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
	r.HandleFunc("/resource/{room}/{session}", proxyHandler)
	r.HandleFunc("/resource/{room}/{session}/layer", proxyHandler)
	r.PathPrefix("/").Handler(http.StripPrefix("/", http.FileServer(http.FS(assets))))
	r.Use(loggingMiddleware)
	r.Use(mux.CORSMethodMiddleware(r))
	panic(http.ListenAndServe(config.ListenAddr, r))
}

func loggingMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		header, _ := json.Marshal(r.Header)
		log.Printf("%s %s %s", r.Method, r.RequestURI, header)
		next.ServeHTTP(w, r)
	})
}

func whipHandler(w http.ResponseWriter, r *http.Request) {
	room := extractRequestRoom(r)
	ownership, err := storage.GetRoomOwnership(r.Context(), room)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	if ownership != nil {
		http.Error(w, fmt.Sprintf("room has been used,node %s", ownership.Addr), http.StatusInternalServerError)
		return
	}
	next, err := loadBalancing.Next(r.Context(), storage)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	doProxy(w, r, next.Addr)
}

func whepHandler(w http.ResponseWriter, r *http.Request) {
	room := extractRequestRoom(r)
	ownership, err := storage.GetRoomOwnership(r.Context(), room)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	fmt.Printf("%+v\n", ownership)

	res, err := request(http.MethodGet, "http://"+ownership.Addr+"/admin/infos", nil)
	if err != nil {
		panic(err)
	}

	type Info struct {
		Id                    string `json:"id"`
		PublishLeaveTime      int    `json:"publishLeaveTime"`
		SubscribeSessionInfos []struct {
			Id string `json:"id"`
		} `json:"subscribeSessionInfos"`
	}

	infos := []Info{}
	if err := json.NewDecoder(res.Body).Decode(&infos); err != nil {
		panic(err)
	}

	var info Info
	for _, i := range infos {
		if i.Id == room {
			info = i
		}
	}

	max, _ := strconv.Atoi(ownership.Metadata)
	if len(info.SubscribeSessionInfos) >= max {
		next, err := loadBalancing.Next(r.Context(), storage)
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}

		fmt.Println("P2P => SFU")

		// P2P => SFU
		apiUrl := "http://" + ownership.Addr + "/admin/reforward/" + room
		whipUrl := "http://" + next.Addr + "/whip/" + room + "0"
		whepUrl := "http://" + next.Addr + "/whep/" + room + "0"
		if _, err := request(http.MethodPost, apiUrl, &map[string]string{
			"targetUrl": whipUrl,
		}); err != nil {
			panic(err)
		}

		time.Sleep(3 * time.Second)
		r.URL, _ = url.Parse(whepUrl)
		fmt.Println("P2P => SFU END")
		doProxy(w, r, next.Addr)
	} else {
		doProxy(w, r, ownership.Addr)
	}
}

func proxyHandler(w http.ResponseWriter, r *http.Request) {
	room := extractRequestRoom(r)
	ownership, err := storage.GetRoomOwnership(r.Context(), room)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	if ownership == nil {
		http.Error(w, "the room does not exist", http.StatusNotFound)
		return
	}
	doProxy(w, r, ownership.Addr)
}

func extractRequestRoom(r *http.Request) string {
	vars := mux.Vars(r)
	return vars["room"]
}

func doProxy(w http.ResponseWriter, r *http.Request, node string) {
	log.Printf("request URI : %s, Handler Node : %s", r.RequestURI, node)
	proxy := httputil.ReverseProxy{
		Director: func(req *http.Request) {
			req.URL.Scheme = "http"
			req.URL.Host = node
			req.Host = node
		},
	}
	proxy.ServeHTTP(w, r)
}
