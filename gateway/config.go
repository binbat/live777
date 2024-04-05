package main

import "github.com/BurntSushi/toml"

type Config struct {
	LoadBalancing string
	ListenAddr    string
	Model         string
	Addr          string
}

func ParseConfig(path string) *Config {
	cfg := &Config{
		LoadBalancing: "random",
		ListenAddr:    ":8080",
		Model:         "RedisStandalone",
		Addr:          "localhost:6379",
	}
	_, err := toml.DecodeFile(path, cfg)
	if err != nil {
		_, err := toml.DecodeFile("config.toml", cfg)
		if err != nil {
			_, err := toml.DecodeFile("/etc/live777/gateway/config.toml", cfg)
			if err != nil {
				panic(err)
			}
		}
	}
	return cfg
}
