package main

import (
	"bytes"
	"encoding/json"
	"net/http"
)

func request(method string, url string, body interface{}) (*http.Response, error) {
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
	return (&http.Client{}).Do(req)
}
