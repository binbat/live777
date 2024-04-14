package main

import (
	"bytes"
	"io"
	"log/slog"
	"net/http"
)

type RequestDump struct {
	Method     string              `json:"method"`
	URI        string              `json:"uri"`
	Proto      string              `json:"proto"`
	Headers    map[string][]string `json:"headers"`
	Body       string              `json:"body"`
	Host       string              `json:"host"`
	RemoteAddr string              `json:"remote_addr"`
}

func buildRequestDump(req *http.Request) RequestDump {
	var body []byte
	if req.Body != nil {
		body, _ = io.ReadAll(req.Body)
		req.Body = io.NopCloser(bytes.NewBuffer(body))
	}
	return RequestDump{
		Method:     req.Method,
		URI:        req.URL.String(),
		Proto:      req.Proto,
		Headers:    req.Header,
		Body:       string(body),
		Host:       req.Host,
		RemoteAddr: req.RemoteAddr,
	}
}

type ResponseDump struct {
	StatusCode int                 `json:"status_code"`
	Status     string              `json:"status"`
	Proto      string              `json:"proto"`
	Headers    map[string][]string `json:"headers"`
	Body       string              `json:"body"`
}

func buildResponseDump(resp *http.Response) ResponseDump {
	var body []byte
	if resp.Body != nil {
		body, _ = io.ReadAll(resp.Body)
		resp.Body = io.NopCloser(bytes.NewBuffer(body))
	}
	return ResponseDump{
		StatusCode: resp.StatusCode,
		Status:     resp.Status,
		Proto:      resp.Proto,
		Headers:    resp.Header,
		Body:       string(body),
	}
}

type loggingTransport struct {
	operation string
	transport http.RoundTripper
}

func (t *loggingTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	requestDump := buildRequestDump(req)
	resp, err := t.transport.RoundTrip(req)
	var responseDump ResponseDump
	if err == nil {
		responseDump = buildResponseDump(resp)
	}
	slog.Debug("http request",
		"operation", t.operation,
		"request", requestDump,
		"response", responseDump)
	return resp, err
}

func loggingMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		slog.Info("server http request",
			"method", r.Method,
			"uri", r.RequestURI,
			"header", r.Header)
		next.ServeHTTP(w, r)
	})
}
