package proxy

import (
	"context"
	"net"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/ictrektech/v-router/internal/registry"
)

func requestInUpstream(r *http.Request, u registry.Upstream) bool {
	base, err := url.Parse(u.BaseURL)
	if err != nil || base.Scheme != r.URL.Scheme || !strings.EqualFold(base.Host, r.URL.Host) {
		return false
	}
	prefix := strings.TrimRight(base.Path, "/")
	return r.URL.Path == prefix || strings.HasPrefix(r.URL.Path, prefix+"/")
}
func privateIP(ip net.IP) bool {
	if ip.IsPrivate() || ip.IsLoopback() || ip.IsLinkLocalUnicast() || ip.IsUnspecified() {
		return true
	}
	v4 := ip.To4()
	return v4 != nil && v4[0] == 100 && v4[1] >= 64 && v4[1] <= 127
}

// A LAN address is a resource location, not proof of cloud ownership. Private
// external providers require an administrator's origin-bound cloud declaration.
func privateTarget(ctx context.Context, target *url.URL) bool {
	host := strings.TrimSuffix(strings.ToLower(target.Hostname()), ".")
	if ip := net.ParseIP(strings.Split(host, "%")[0]); ip != nil {
		return privateIP(ip)
	}
	if !strings.Contains(host, ".") || strings.HasSuffix(host, ".local") || strings.HasSuffix(host, ".lan") || strings.HasSuffix(host, ".internal") {
		return true
	}
	lookup, cancel := context.WithTimeout(ctx, 2*time.Second)
	defer cancel()
	addresses, err := net.DefaultResolver.LookupIPAddr(lookup, host)
	if err != nil || len(addresses) == 0 {
		return true // Unknown DNS scope needs an administrator origin approval.
	}
	for _, address := range addresses {
		if privateIP(address.IP) {
			return true
		}
	}
	return false
}
