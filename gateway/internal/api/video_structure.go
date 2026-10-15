package api

import (
	"net/http"

	"github.com/ictrektech/v-router/internal/proxy"
	"github.com/ictrektech/v-router/internal/registry"
)

const videoStructureSource = "video-structure"

type videoStructureInterface struct {
	method, path, service, summary string
}

var videoStructureInterfaces = []videoStructureInterface{
	{http.MethodPost, "/api/v1/scenes/detect", registry.ServiceVideoStructureDetect, "同步检测视频场景"},
	{http.MethodPost, "/api/v2/scenes/detect/jobs", registry.ServiceVideoStructureDetect, "提交场景检测任务"},
	{http.MethodGet, "/api/v2/scenes/detect/jobs/{job_id}", registry.ServiceVideoStructureDetect, "查询场景检测任务"},
	{http.MethodPost, "/api/v2/video/structure/jobs", registry.ServiceVideoStructureStructure, "提交视频结构化任务"},
	{http.MethodGet, "/api/v2/video/structure/jobs/{job_id}", registry.ServiceVideoStructureStructure, "查询视频结构化任务"},
	{http.MethodGet, "/api/v2/video/structure/jobs/{job_id}/artifact", registry.ServiceVideoStructureStructure, "下载视频结构化产物"},
	{http.MethodDelete, "/api/v2/video/structure/jobs/{job_id}", registry.ServiceVideoStructureStructure, "释放视频结构化产物"},
	{http.MethodPost, "/api/v2/video/describe/jobs", registry.ServiceVideoStructureStructure, "提交场景描述任务"},
	{http.MethodGet, "/api/v2/video/describe/jobs/{job_id}", registry.ServiceVideoStructureStructure, "查询场景描述任务"},
}

func (s *Server) videoStructureRoutes() []apiRouteSpec {
	routes := []apiRouteSpec{{
		Method: "GET", Path: "/api/v1/video-structure/status", Group: "video-structure", Auth: "inference",
		Summary: "视频结构化服务发现状态", Desc: "报告 scene-detect 与 structure 是否已由 V-Router 定期发现；服务未安装或不可达是正常状态。", Handler: s.handleVideoStructureStatus,
	}}
	for _, route := range videoStructureInterfaces {
		service := route.service
		routes = append(routes, apiRouteSpec{
			Method: route.method, Path: route.path, Group: "video-structure", Auth: "inference", Summary: route.summary,
			Desc:    "保持视频结构化原始路径、请求体、状态码、响应头和响应体透传；V-Router 只暴露业务接口，不把该服务加入基础模型池。",
			Handler: func(w http.ResponseWriter, r *http.Request) { s.forwardVideoStructure(w, r, service) },
		})
	}
	return routes
}

func (s *Server) handleVideoStructureStatus(w http.ResponseWriter, _ *http.Request) {
	_, detect := s.reg.Service(registry.ServiceVideoStructureDetect)
	_, structure := s.reg.Service(registry.ServiceVideoStructureStructure)
	writeJSON(w, http.StatusOK, map[string]any{
		"available": detect && structure,
		"services":  map[string]bool{"scene_detect": detect, "structure": structure},
	})
}

func (s *Server) forwardVideoStructure(w http.ResponseWriter, r *http.Request, service string) {
	baseURL, ok := s.reg.Service(service)
	if !ok {
		writeRequestError(w, r, http.StatusServiceUnavailable, "video structure service is not available")
		return
	}
	w.Header().Set("X-VRouter-Service", service)
	proxy.To(w, r, registry.Upstream{ID: service, BaseURL: baseURL, Kind: registry.KindLocal, Protocol: "video_structure", Healthy: true}, r.URL.Path)
}
