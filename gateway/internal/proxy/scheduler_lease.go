package proxy

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"sync"
	"time"
)

type permitLease struct {
	cfg        *SchedulerConfig
	id         string
	proof      string
	ctx        context.Context
	cancel     context.CancelFunc
	done       chan struct{}
	stopOnce   sync.Once
	finishOnce sync.Once
}

var permitRenewInterval = 5 * time.Second

func acquirePermit(ctx context.Context, cfg *SchedulerConfig, executor, upstream, model string) (*permitLease, error) {
	proof := make([]byte, 32)
	if _, err := rand.Read(proof); err != nil {
		return nil, err
	}
	token := hex.EncodeToString(proof)
	lease, err := acquireLease(ctx, cfg, "/permits", map[string]string{"executor_id": executor, "upstream_url": upstream, "model_id": model, "execution_token": token})
	if err == nil {
		lease.proof = token
	}
	return lease, err
}
func acquireDelegation(ctx context.Context, cfg *SchedulerConfig, executor, upstream, model string, proof delegation) (*permitLease, error) {
	secret := make([]byte, 32)
	if _, err := rand.Read(secret); err != nil { return nil, err }
	token := hex.EncodeToString(secret)
	lease, err := acquireLease(ctx, cfg, "/delegations/verify", map[string]string{"task_id": proof.TaskID, "owner_id": proof.OwnerID, "executor_id": executor, "upstream_url": upstream, "model_id": model, "execution_token": token})
	if err == nil { lease.proof = token }
	return lease, err
}
func acquireLease(ctx context.Context, cfg *SchedulerConfig, path string, body any) (*permitLease, error) {
	key := make([]byte, 16)
	if _, err := rand.Read(key); err != nil {
		return nil, err
	}
	data, err := schedulerCall(ctx, cfg, path, body, hex.EncodeToString(key))
	if err != nil {
		return nil, err
	}
	var id string
	if json.Unmarshal(data["permit_id"], &id) != nil || id == "" {
		return nil, &AdmissionError{503, "invalid local permit"}
	}
	return resumePermit(cfg, id), nil
}
func resumePermit(cfg *SchedulerConfig, id string) *permitLease {
	ctx, cancel := context.WithCancel(context.Background())
	lease := &permitLease{cfg: cfg, id: id, ctx: ctx, cancel: cancel, done: make(chan struct{})}
	go func() {
		ticker := time.NewTicker(permitRenewInterval)
		defer ticker.Stop()
		for {
			select {
			case <-lease.done:
				return
			case <-ctx.Done():
				return
			case <-ticker.C:
				if _, err := schedulerCall(ctx, cfg, "/permits/"+id+"/renew", map[string]any{}, ""); err != nil {
					cancel()
					return
				}
			}
		}
	}()
	return lease
}
func (lease *permitLease) stop() {
	lease.stopOnce.Do(func() { close(lease.done); lease.cancel() })
}
func (lease *permitLease) finish(stopped bool, evidence string) {
	lease.finishOnce.Do(func() {
		lease.stop()
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		_, _ = schedulerCall(ctx, lease.cfg, "/permits/"+lease.id+"/release", map[string]any{"stopped": stopped, "evidence": evidence}, "")
	})
}
