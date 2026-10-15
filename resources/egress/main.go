// egress gives a vfkit NIC a user-mode IPv4 NAT and metadata-only flow journal.
// No host interfaces, routes, firewall changes, or privileged sockets are needed.
package main

import (
	"bytes"
	"context"
	"encoding/binary"
	"encoding/json"
	"flag"
	"fmt"
	"net"
	"net/http"
	"os"
	"os/exec"
	"os/signal"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/containers/gvisor-tap-vsock/pkg/transport"
	"github.com/containers/gvisor-tap-vsock/pkg/types"
	"github.com/containers/gvisor-tap-vsock/pkg/virtualnetwork"
	"github.com/google/gopacket"
	"github.com/google/gopacket/layers"
)

type flow struct {
	Agent       string    `json:"agent"`
	Wake        string    `json:"wake"`
	Destination string    `json:"destination"`
	Port        uint16    `json:"port"`
	Protocol    string    `json:"protocol"`
	DNS         string    `json:"dns,omitempty"`
	First       time.Time `json:"first"`
	Last        time.Time `json:"last"`
	Out         uint64    `json:"outBytes"`
	In          uint64    `json:"inBytes"`
	Packets     uint64    `json:"packets"`
	Blocked     bool      `json:"blocked,omitempty"`
}

// One sandbox command can cold-boot the same NIC twice (home measurement and export).
// vfkit binds a fresh host socket each boot; retain the NAT but accept its new VFKT peer.
type restartableNIC struct {
	*net.UnixConn
	mu   sync.Mutex
	peer *net.UnixAddr
}

func (n *restartableNIC) Read(b []byte) (int, error) {
	for {
		count, peer, err := n.UnixConn.ReadFromUnix(b)
		if err != nil {
			return count, err
		}
		if count == 4 && bytes.Equal(b[:count], []byte("VFKT")) {
			n.mu.Lock()
			n.peer = peer
			n.mu.Unlock()
			continue
		}
		return count, nil
	}
}
func (n *restartableNIC) Write(b []byte) (int, error) {
	n.mu.Lock()
	peer := n.peer
	n.mu.Unlock()
	return n.UnixConn.WriteToUnix(b, peer)
}
func (n *restartableNIC) RemoteAddr() net.Addr { n.mu.Lock(); defer n.mu.Unlock(); return n.peer }

type journal struct {
	net.Conn
	mu                sync.Mutex
	flows             map[string]*flow
	file              *os.File
	agent, wake, host string
	port              uint16
	hostPort          uint16
	attached          []*net.IPNet
}

func (j *journal) record(b []byte, outbound bool) bool {
	p := gopacket.NewPacket(b, layers.LayerTypeEthernet, gopacket.NoCopy)
	ipLayer := p.Layer(layers.LayerTypeIPv4)
	if ipLayer == nil {
		return p.Layer(layers.LayerTypeARP) != nil
	} // IPv6 has no alternate route.
	ip := ipLayer.(*layers.IPv4)
	dest := ip.DstIP
	port := uint16(0)
	proto := ip.Protocol.String()
	if !outbound {
		dest = ip.SrcIP
	}
	if l := p.Layer(layers.LayerTypeTCP); l != nil {
		t := l.(*layers.TCP)
		port = uint16(t.DstPort)
		if !outbound {
			port = uint16(t.SrcPort)
		}
	}
	if l := p.Layer(layers.LayerTypeUDP); l != nil {
		u := l.(*layers.UDP)
		port = uint16(u.DstPort)
		if !outbound {
			port = uint16(u.SrcPort)
		}
	}
	// Public internet and exactly one host API. DHCP remains local to this stack.
	dhcp := outbound && ip.Protocol == layers.IPProtocolUDP && port == 67 && (dest.Equal(net.IPv4bcast) || dest.String() == "192.168.64.254")
	private := dest.IsPrivate() || dest.IsLoopback() || dest.IsLinkLocalUnicast() || dest.IsMulticast() || dest.IsUnspecified() || dest[0] == 0 || dest[0] >= 224 || (dest[0] == 100 && dest[1] >= 64 && dest[1] <= 127)
	for _, subnet := range j.attached {
		private = private || subnet.Contains(dest)
	}
	allowed := !private || (dest.String() == j.host && proto == "TCP" && port == j.port) || dhcp
	// Fragments could hide a private destination port; the stack reassembles public packets only.
	if ip.FragOffset != 0 && private {
		allowed = false
	}
	dns := ""
	if l := p.Layer(layers.LayerTypeDNS); l != nil {
		d := l.(*layers.DNS)
		for _, q := range d.Questions {
			if dns != "" {
				dns += ","
			}
			dns += string(q.Name)
		}
	}
	blocked := outbound && !allowed
	key := fmt.Sprintf("%s/%d/%s/%s/%t", dest, port, proto, dns, blocked)
	now := time.Now().UTC()
	j.mu.Lock()
	defer j.mu.Unlock()
	f := j.flows[key]
	if f == nil {
		f = &flow{Agent: j.agent, Wake: j.wake, Destination: dest.String(), Port: port, Protocol: proto, DNS: dns, First: now, Blocked: blocked}
		j.flows[key] = f
	}
	f.Last = now
	f.Packets++
	if outbound {
		f.Out += uint64(len(b))
	} else {
		f.In += uint64(len(b))
	}
	// Bound memory even during a scan; flushing retains every destination.
	if len(j.flows) >= 4096 {
		j.flushLocked()
	}
	return allowed || !outbound
}
func (j *journal) flushLocked() {
	e := json.NewEncoder(j.file)
	for k, f := range j.flows {
		if err := e.Encode(f); err != nil {
			panic(err)
		}
		delete(j.flows, k)
	}
	if err := j.file.Sync(); err != nil {
		panic(err)
	}
}
func (j *journal) flush() { j.mu.Lock(); defer j.mu.Unlock(); j.flushLocked() }

// The guest-facing API port maps to a distinct untrusted host listener.
// RFC 1624 incremental checksum update; HTTPS/public traffic remains byte-exact.
func (j *journal) translate(b []byte, outbound bool) {
	if j.port == j.hostPort {
		return
	}
	p := gopacket.NewPacket(b, layers.LayerTypeEthernet, gopacket.NoCopy)
	il, tl := p.Layer(layers.LayerTypeIPv4), p.Layer(layers.LayerTypeTCP)
	if il == nil || tl == nil {
		return
	}
	ip, t := il.(*layers.IPv4), tl.(*layers.TCP)
	offset := 0
	old, new := j.hostPort, j.port
	if outbound {
		if ip.DstIP.String() != j.host {
			return
		}
		offset = 2
		old, new = j.port, j.hostPort
	} else if ip.SrcIP.String() != j.host {
		return
	}
	contents := t.LayerContents()
	if len(contents) < 18 || binary.BigEndian.Uint16(contents[offset:]) != old {
		return
	}
	checksum := binary.BigEndian.Uint16(contents[16:])
	sum := uint32(^checksum) + uint32(^old) + uint32(new)
	for sum>>16 != 0 {
		sum = (sum & 0xffff) + (sum >> 16)
	}
	binary.BigEndian.PutUint16(contents[offset:], new)
	binary.BigEndian.PutUint16(contents[16:], ^uint16(sum))
}
func (j *journal) Read(b []byte) (int, error) {
	for {
		n, e := j.Conn.Read(b)
		if e != nil {
			return n, e
		}
		if j.record(b[:n], true) {
			j.translate(b[:n], true)
			return n, nil
		}
	}
}
func (j *journal) Write(b []byte) (int, error) {
	j.translate(b, false)
	j.record(b, false)
	return j.Conn.Write(b)
}

// Independent owner-loss backstop: an uncatchable coordinator exit must not leave its VM executing.
func retainOnOwnerLoss(owner int, endpoint, hold, hook string, j *journal) {
	t := time.NewTicker(time.Second)
	defer t.Stop()
	for range t.C {
		if os.Getppid() == owner {
			continue
		}
		first := false
		if f, e := os.OpenFile(hold, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600); e == nil {
			first = true
			_ = json.NewEncoder(f).Encode(map[string]any{"reason": "cohort owner exited", "time": time.Now().Unix(), "ownerPid": owner})
			_ = f.Sync()
			_ = f.Close()
		}
		client := &http.Client{Timeout: 2 * time.Second}
		response, err := client.Post(endpoint+"/vm/state", "application/json", bytes.NewBufferString(`{"state":"Pause"}`))
		if err == nil {
			response.Body.Close()
		}
		j.flush()
		if first && hook != "" {
			_ = exec.Command("python3", hook, "orphan", hold).Start()
		}
		_ = syscall.Kill(os.Getpid(), syscall.SIGSTOP)
		return
	}
}

func main() {
	socket := flag.String("socket", "", "vfkit unix datagram socket")
	log := flag.String("log", "", "append-only metadata journal")
	agent := flag.String("agent", "", "host-assigned identity")
	wake := flag.String("wake", "", "host-assigned wake")
	host := flag.String("api-host", "192.168.64.1", "guest-facing host API address")
	port := flag.Int("api-port", 0, "guest-facing API TCP port")
	hostPort := flag.Int("host-api-port", 0, "separate untrusted host listener port (required)")
	control := flag.String("vm-control", "", "loopback vfkit control endpoint")
	hold := flag.String("owner-hold", "", "durable operator hold on owner exit")
	hook := flag.String("owner-exit-hook", "", "host-owned exit callback")
	mac := flag.String("mac", "02:00:00:00:00:02", "guest MAC")
	flag.Parse()
	if *socket == "" || *log == "" || *agent == "" || *port < 1 || *port > 65535 || *hostPort < 1 || *hostPort > 65535 || *hostPort == *port {
		panic("socket, log, agent, and api-port required")
	}
	f, e := os.OpenFile(*log, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0600)
	if e != nil {
		panic(e)
	}
	defer f.Close()
	c, e := net.ListenUnixgram("unixgram", &net.UnixAddr{Name: *socket, Net: "unixgram"})
	if e != nil {
		panic(e)
	}
	defer os.Remove(*socket)
	vn, e := virtualnetwork.New(&types.Configuration{
		MTU: 1500, Subnet: "192.168.64.0/24", GatewayIP: "192.168.64.254", GatewayMacAddress: "5a:94:ef:e4:0c:dd",
		DHCPStaticLeases: map[string]string{"192.168.64.2": *mac},
		NAT:              map[string]string{*host: "127.0.0.1"}, GatewayVirtualIPs: []string{*host}, Protocol: types.VfkitProtocol,
	})
	if e != nil {
		panic(e)
	}
	fmt.Println("ready")
	conn, e := transport.AcceptVfkit(c)
	if e != nil {
		panic(e)
	}
	nic := &restartableNIC{UnixConn: c, peer: conn.RemoteAddr().(*net.UnixAddr)}
	j := &journal{Conn: nic, file: f, flows: make(map[string]*flow), agent: *agent, wake: *wake, host: *host, port: uint16(*port), hostPort: uint16(*hostPort)}
	addresses, err := net.InterfaceAddrs()
	if err != nil {
		panic(err)
	}
	for _, address := range addresses {
		ip, subnet, err := net.ParseCIDR(address.String())
		if err == nil && ip.To4() != nil {
			j.attached = append(j.attached, subnet)
		}
	}
	defer j.flush()
	if *control != "" && *hold != "" {
		if !strings.HasPrefix(*control, "http://127.0.0.1:") {
			panic("control endpoint must be loopback")
		}
		go retainOnOwnerLoss(os.Getppid(), *control, *hold, *hook, j)
	}
	flush := make(chan os.Signal, 1)
	signal.Notify(flush, syscall.SIGUSR1)
	go func() {
		for range flush {
			j.flush()
			if e := os.WriteFile(*log+".flushed", []byte(time.Now().UTC().Format(time.RFC3339Nano)), 0600); e != nil {
				panic(e)
			}
		}
	}()
	ctx, cancel := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer cancel()
	go func() { <-ctx.Done(); conn.Close() }()
	go func() {
		t := time.NewTicker(5 * time.Second)
		defer t.Stop()
		for {
			select {
			case <-t.C:
				j.flush()
			case <-ctx.Done():
				return
			}
		}
	}()
	if e = vn.AcceptVfkit(ctx, j); e != nil && ctx.Err() == nil {
		panic(e)
	}
}
