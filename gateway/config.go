package main

import "github.com/BurntSushi/toml"

type Config struct {
	ListenAddr              string
	Model                   string
	Addr                    string
	Level                   string
	ReforwardCheckFrequency int
	CheckReforwardTickTime  int
}

func ParseConfig(path string) *Config {
	cfg := &Config{
		ListenAddr:              ":8080",
		Model:                   "RedisStandalone",
		Addr:                    "redis://localhost:6379",
		Level:                   "DEBUG",
		ReforwardCheckFrequency: 5,
		CheckReforwardTickTime:  3000,
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
