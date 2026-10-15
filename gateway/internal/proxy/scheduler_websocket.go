package proxy

import (
	"context"
	"encoding/binary"
	"io"
	"sync"
	"sync/atomic"
)

// Observe only frame headers and the small close control frame. Audio/model
// payloads remain streaming and are never accumulated here. An approved
// connection-scoped executor sends close(1000) after its compute has stopped;
// a client disconnect or TCP EOF alone does not establish that proof.
type serverCloseObserver struct {
	header       []byte
	headerSize   int
	remaining    uint64
	opcode       byte
	closeFrame   bool
	closePayload []byte
	normal       *atomic.Bool
}

func (o *serverCloseObserver) feed(data []byte) {
	for len(data) > 0 {
		if o.headerSize == 0 {
			o.headerSize = 2
		}
		if len(o.header) < o.headerSize {
			n := o.headerSize - len(o.header)
			if n > len(data) {
				n = len(data)
			}
			o.header = append(o.header, data[:n]...)
			data = data[n:]
			if len(o.header) < o.headerSize {
				return
			}
			if o.headerSize == 2 {
				length := o.header[1] & 127
				if length == 126 {
					o.headerSize += 2
				}
				if length == 127 {
					o.headerSize += 8
				}
				if o.header[1]&128 != 0 {
					o.headerSize += 4
				}
				if len(o.header) < o.headerSize {
					continue
				}
			}
			length := uint64(o.header[1] & 127)
			if length == 126 {
				length = uint64(binary.BigEndian.Uint16(o.header[2:4]))
			}
			if length == 127 {
				length = binary.BigEndian.Uint64(o.header[2:10])
			}
			o.remaining = length
			o.opcode = o.header[0] & 15
			o.closeFrame = o.opcode == 8 && o.header[0]&128 != 0 && o.header[1]&128 == 0 && length <= 125
		}
		n := uint64(len(data))
		if n > o.remaining {
			n = o.remaining
		}
		if o.closeFrame {
			o.closePayload = append(o.closePayload, data[:int(n)]...)
		}
		data = data[int(n):]
		o.remaining -= n
		if o.remaining == 0 {
			if o.closeFrame && len(o.closePayload) >= 2 && binary.BigEndian.Uint16(o.closePayload[:2]) == 1000 {
				o.normal.Store(true)
			}
			o.header = nil
			o.headerSize = 0
			o.closePayload = nil
			o.closeFrame = false
		}
	}
}

type permitDuplex struct {
	io.ReadWriteCloser
	normal    atomic.Bool
	observer  serverCloseObserver
	once      sync.Once
	done      chan struct{}
	finish    func(bool)
	proved    func()
	proofOnce sync.Once
	closeErr  error
	remaining int64
}

func newPermitDuplex(body io.ReadWriteCloser, ctx context.Context, limit int64, finish func(bool), proved func()) *permitDuplex {
	wrapper := &permitDuplex{ReadWriteCloser: body, done: make(chan struct{}), finish: finish, remaining: limit, proved: proved}
	wrapper.observer.normal = &wrapper.normal
	go func() {
		select {
		case <-ctx.Done():
			_ = wrapper.Close()
		case <-wrapper.done:
		}
	}()
	return wrapper
}
func (b *permitDuplex) Read(data []byte) (int, error) {
	n, err := b.ReadWriteCloser.Read(data)
	b.observer.feed(data[:n])
	if b.normal.Load() && b.proved != nil {
		b.proofOnce.Do(b.proved)
	}
	return n, err
}
func (b *permitDuplex) Write(data []byte) (int, error) {
	if b.remaining > 0 {
		if int64(len(data)) > b.remaining {
			_ = b.Close()
			return 0, io.ErrShortWrite
		}
		b.remaining -= int64(len(data))
		if b.remaining == 0 {
			b.remaining = -1
		}
	} else if b.remaining < 0 {
		_ = b.Close()
		return 0, io.ErrShortWrite
	}
	return b.ReadWriteCloser.Write(data)
}
func (b *permitDuplex) Close() error {
	b.once.Do(func() { close(b.done); b.closeErr = b.ReadWriteCloser.Close(); b.finish(b.normal.Load()) })
	return b.closeErr
}
