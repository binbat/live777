package main

import (
	"bytes"
	"io"
	"log/slog"
	"net/http"
	"time"
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
	start := time.Now().UnixMilli()
	resp, err := t.transport.RoundTrip(req)
	end := time.Now().UnixMilli()
	var responseDump ResponseDump
	if err == nil {
		responseDump = buildResponseDump(resp)
	}
	slog.Debug("http client request",
		"operation", t.operation,
		"request", requestDump,
		"response", responseDump,
		"take", end-start,
	)
	return resp, err
}

type responseWriter struct {
	w      http.ResponseWriter
	status int
	body   *bytes.Buffer
}

func (r *responseWriter) Header() http.Header {
	return r.w.Header()
}

func (r *responseWriter) Write(i []byte) (int, error) {
	write, err := r.w.Write(i)
	r.body.Write(i)
	return write, err
}

func (r *responseWriter) WriteHeader(statusCode int) {
	r.status = statusCode
	r.w.WriteHeader(statusCode)
}

func loggingMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requestDump := buildRequestDump(r)
		writer := &responseWriter{w: w, body: bytes.NewBufferString("")}
		start := time.Now().UnixMilli()
		next.ServeHTTP(writer, r)
		end := time.Now().UnixMilli()
		responseDump := ResponseDump{
			StatusCode: writer.status,
			Status:     http.StatusText(writer.status),
			Proto:      r.Proto,
			Headers:    w.Header(),
			Body:       writer.body.String(),
		}
		slog.Info("http server request",
			"request", requestDump,
			"response", responseDump,
			"take", end-start,
		)
	})
}
