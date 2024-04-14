package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"
)

const (
	RTCPeerConnectionStateUnspecified = iota
	RTCPeerConnectionStateNew
	RTCPeerConnectionStateConnecting
	RTCPeerConnectionStateConnected
	RTCPeerConnectionStateDisconnected
	RTCPeerConnectionStateFailed
	RTCPeerConnectionStateClosed
)

type Node struct {
	Addr     string       `json:"addr"`
	Metadata NodeMetaData `json:"metadata"`
}

type NodeMetaData struct {
	PubMax                   uint64  `json:"pubMax"`
	SubMax                   uint64  `json:"subMax"`
	ReforwardMaximumIdleTime uint64  `json:"reforwardMaximumIdleTime"`
	ReforwardCascade         bool    `json:"ReforwardCascade"`
	Authorization            *string `json:"authorization,omitempty"`
	AdminAuthorization       *string `json:"adminAuthorization,omitempty"`
}

type RoomInfo struct {
	Id                    string        `json:"id"`
	CreateTime            int64         `json:"createTime"`
	PublishLeaveTime      int           `json:"publishLeaveTime"`
	SubscribeLeaveTime    int           `json:"subscribeLeaveTime"`
	PublishSessionInfo    *SessionInfo  `json:"publishSessionInfo"`
	SubscribeSessionInfos []SessionInfo `json:"subscribeSessionInfos"`
}

type SessionInfo struct {
	Id           string         `json:"id"`
	CreateTime   int64          `json:"createTime"`
	ConnectState int            `json:"connectState"`
	Reforward    *ReforwardInfo `json:"reforward,omitempty"`
}

type ReforwardInfo struct {
	TargetUrl   string `json:"targetUrl"`
	ResourceUrl string `json:"resourceUrl"`
}

func (reforward ReforwardInfo) ParseNodeAndRoom() (string, string) {
	targetUrl := reforward.TargetUrl
	parse, _ := url.Parse(targetUrl)
	split := strings.Split(parse.RequestURI(), "/")
	return parse.Host, split[len(split)-1]
}

const metricsPrefix = "live777_"

type NodeMetrics struct {
	Room      uint64 `json:"room"`
	Publish   uint64 `json:"publish"`
	Subscribe uint64 `json:"subscribe"`
	Reforward uint64 `json:"reforward"`
}

func (node *Node) GetRoomInfo(room string) (*RoomInfo, error) {
	infos, err := node.GetRoomInfos(room)
	if err != nil {
		return nil, err
	}
	if len(infos) == 0 {
		return nil, nil
	}
	return &infos[0], nil
}

func (node *Node) GetRoomInfos(room ...string) ([]RoomInfo, error) {
	response, err := request("GET", fmt.Sprintf("http://%s/admin/infos?rooms=%s", node.Addr, strings.Join(room, ",")), node.Metadata.AdminAuthorization, nil)
	if err != nil {
		return nil, err
	}
	body := response.Body
	defer body.Close()
	infos := make([]RoomInfo, 0)
	err = json.NewDecoder(body).Decode(&infos)
	return infos, err
}

func (node *Node) GetMetrics() (*NodeMetrics, error) {
	response, err := request("GET", fmt.Sprintf("http://%s/metrics", node.Addr), nil, nil)
	if err != nil {
		return nil, err
	}
	body := response.Body
	defer body.Close()
	metrics := &NodeMetrics{}
	scanner := bufio.NewScanner(body)
	for scanner.Scan() {
		line := scanner.Text()
		if strings.HasPrefix(line, metricsPrefix) {
			text := line[len(metricsPrefix):]
			split := strings.Split(text, " ")
			val, err := strconv.ParseUint(split[1], 10, 16)
			if err != nil {
				return nil, err
			}
			switch split[0] {
			case "room":
				metrics.Room = val
			case "publish":
				metrics.Publish = val
			case "subscribe":
				metrics.Subscribe = val
			case "reforward":
				metrics.Reforward = val
			}
		}
	}
	return metrics, nil
}

func (node *Node) Reforward(targetNode Node, nodeRoom, targetRoom string) error {
	type ReforwardReq struct {
		TargetUrl          string  `json:"targetUrl"`
		AdminAuthorization *string `json:"adminAuthorization,omitempty"`
	}
	response, err := request("POST", fmt.Sprintf("http://%s/admin/reforward/%s", node.Addr, nodeRoom), node.Metadata.AdminAuthorization, ReforwardReq{
		TargetUrl:          fmt.Sprintf("http://%s/whip/%s", targetNode.Addr, targetRoom),
		AdminAuthorization: targetNode.Metadata.AdminAuthorization,
	})
	if err != nil {
		return err
	}
	response.Body.Close()
	return nil
}

func (node *Node) ResourceDelete(room, session string) error {
	response, err := request("DELETE", fmt.Sprintf("http://%s/resource/%s/%s", node.Addr, room, session), node.Metadata.Authorization, nil)
	if err != nil {
		return err
	}
	response.Body.Close()
	return nil
}

func request(method, url string, authorization *string, body interface{}) (*http.Response, error) {
	buf := new(bytes.Buffer)
	if body != nil {
		if err := json.NewEncoder(buf).Encode(body); err != nil {
			return nil, err
		}
	}
	req, err := http.NewRequest(method, url, buf)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")
	if authorization != nil {
		req.Header.Set("Authorization", *authorization)
	}
	response, err := (&http.Client{
		Transport: &loggingTransport{
			operation: "CLIENT",
			transport: http.DefaultTransport,
		},
	}).Do(req)
	if err != nil {
		return nil, err
	}
	if response.StatusCode != http.StatusOK {
		body := response.Body
		defer body.Close()
		data, err := io.ReadAll(body)
		if err != nil {
			return nil, err
		}
		return nil, errors.New(string(data))
	}
	return response, nil
}
