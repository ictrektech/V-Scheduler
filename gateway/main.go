// Command v-router is the VOS local AI API gateway. It aggregates local
// inference services, the company cloud aggregation platform, and manually
// added OpenAI-compatible endpoints behind one OpenAI-compatible API.
package main

import (
	"context"
	"flag"
	"log"
	"net/http"
	"os"
	"os/signal"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/ictrektech/v-router/internal/api"
	"github.com/ictrektech/v-router/internal/proxy"
	"github.com/ictrektech/v-router/internal/registry"
)

// Overridden at build time via -ldflags; at runtime the packaged
// VOS_APP_VERSION env (rendered by package.sh) wins.
var version = "dev"

func envOr(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}

func main() {
	if v := os.Getenv("VOS_APP_VERSION"); v != "" {
		version = v
	}
	addr := flag.String("addr", envOr("V_ROUTER_ADDR", ":8080"), "listen address")
	statePath := flag.String("state", envOr("V_ROUTER_STATE_PATH", "/data/state.json"), "state file path")
	modelHubURL := flag.String("model-hub", envOr("V_ROUTER_MODEL_HUB_URL", ""), "Model Hub backend URL, empty disables discovery")
	imageStudioURL := flag.String("image-studio", envOr("V_ROUTER_IMAGE_STUDIO_URL", "http://image-studio-backend:5000"), "Image Studio backend URL, empty disables discovery")
	vVoiceURLs := flag.String("v-voice", envOr("V_ROUTER_V_VOICE_URLS", "http://v-voice-backend:7070,http://host.docker.internal:22700"), "comma-separated V-Voice discovery URLs, empty disables discovery")
	vMusicURLs := flag.String("v-music", envOr("V_ROUTER_V_MUSIC_URLS", "http://v-music-backend:7080,http://host.docker.internal:22701"), "comma-separated V-Music discovery URLs, empty disables discovery")
	mlServerURLs := flag.String("ml-server", envOr("V_ROUTER_ML_SERVER_URLS", "http://ml-server:3003,http://host.docker.internal:3003"), "comma-separated ML Server discovery URLs, empty disables discovery")
	videoStructureDetectURLs := flag.String("video-structure-detect", envOr("V_ROUTER_VIDEO_STRUCTURE_DETECT_URLS", "http://video-structure-detect:8910,http://host.docker.internal:48910"), "comma-separated video structure scene-detect URLs")
	videoStructureURLs := flag.String("video-structure", envOr("V_ROUTER_VIDEO_STRUCTURE_URLS", "http://video-structure:8911,http://host.docker.internal:48911"), "comma-separated video structure URLs")
	discoveryInterval := flag.Duration("discovery-interval", discoveryIntervalEnv(), "local app discovery interval")
	flag.Parse()

	proxy.ResumeAsync()

	reg, err := registry.Open(*statePath)
	if err != nil {
		log.Fatalf("open state: %v", err)
	}

	go registry.NewSchedulerOllama(reg,
		envOr("V_SCHEDULER_OLLAMA_QA_URL", "http://v-scheduler-ollama-qa:11535"),
		envOr("V_SCHEDULER_OLLAMA_EMBEDDING_URL", "http://v-scheduler-ollama-embedding:11535"),
		envOr("V_SCHEDULER_OLLAMA_RERANK_URL", "http://v-scheduler-ollama-rerank:11535"), *discoveryInterval).Run(context.Background())

	if *modelHubURL != "" {
		poller := registry.NewModelHubPoller(reg, *modelHubURL, *discoveryInterval)
		go poller.Run(context.Background())
	}
	if *imageStudioURL != "" {
		poller := registry.NewImageStudioPoller(reg, *imageStudioURL, *discoveryInterval)
		go poller.Run(context.Background())
	}
	if *vVoiceURLs != "" {
		poller := registry.NewVVoicePoller(reg, strings.Split(*vVoiceURLs, ","), *discoveryInterval)
		go poller.Run(context.Background())
	}
	if *vMusicURLs != "" {
		poller := registry.NewVMusicPoller(reg, strings.Split(*vMusicURLs, ","), *discoveryInterval)
		go poller.Run(context.Background())
	}
	if *mlServerURLs != "" {
		poller := registry.NewMLServerPoller(reg, strings.Split(*mlServerURLs, ","), *discoveryInterval)
		go poller.Run(context.Background())
	}
	if *videoStructureDetectURLs != "" || *videoStructureURLs != "" {
		poller := registry.NewVideoStructurePoller(reg, strings.Split(*videoStructureDetectURLs, ","), strings.Split(*videoStructureURLs, ","), *discoveryInterval)
		go poller.Run(context.Background())
	}

	srv := &http.Server{
		Addr:              *addr,
		Handler:           api.New(reg, version),
		ReadHeaderTimeout: 10 * time.Second,
		IdleTimeout:       30 * time.Second,
		MaxHeaderBytes:    64 << 10,
	}

	go func() {
		log.Printf("v-router %s listening on %s", version, *addr)
		if err := srv.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			log.Fatalf("listen: %v", err)
		}
	}()

	stop := make(chan os.Signal, 1)
	signal.Notify(stop, syscall.SIGINT, syscall.SIGTERM)
	<-stop
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := srv.Shutdown(ctx); err != nil {
		log.Printf("shutdown: %v", err)
	}
}

func discoveryIntervalEnv() time.Duration {
	if v := os.Getenv("V_ROUTER_DISCOVERY_INTERVAL"); v != "" {
		// Plain integers from configs.yml are seconds; durations like "90s" pass through.
		if n, err := strconv.Atoi(v); err == nil {
			return time.Duration(n) * time.Second
		}
		if d, err := time.ParseDuration(v); err == nil {
			return d
		}
		log.Fatalf("invalid V_ROUTER_DISCOVERY_INTERVAL: %s", v)
	}
	return 30 * time.Second
}
