package vil

import (
	"strings"
	"testing"
)

// mustContain asserts that every needle appears in the haystack manifest.
func mustContain(t *testing.T, haystack string, needles []string) {
	t.Helper()
	for _, n := range needles {
		if !strings.Contains(haystack, n) {
			t.Errorf("manifest missing %q\n--- manifest ---\n%s", n, haystack)
		}
	}
}

func TestFieldHelpers(t *testing.T) {
	if got := String(true); got.Type != "String" || !got.Required {
		t.Fatalf("String(true) = %+v", got)
	}
	if got := Number(false); got.Type != "u64" || got.Required {
		t.Fatalf("Number(false) = %+v", got)
	}
	if got := Boolean(true); got.Type != "bool" {
		t.Fatalf("Boolean type = %q", got.Type)
	}
	if got := Array("string"); got.Type != "Vec<string>" {
		t.Fatalf("Array type = %q", got.Type)
	}
}

func TestHandlerImplBuilders(t *testing.T) {
	if got := Stub(""); got.Mode != "stub" || got.Response != `{"ok": true}` {
		t.Fatalf("Stub default = %+v", got)
	}
	if got := Sidecar("HandleX", "shm"); got.Mode != "sidecar" || got.Function != "HandleX" || got.Protocol != "shm" {
		t.Fatalf("Sidecar = %+v", got)
	}
	if got := Wasm("m.wasm", "handle"); got.Mode != "wasm" || got.Module != "m.wasm" {
		t.Fatalf("Wasm = %+v", got)
	}
}

func TestModeFromEnv(t *testing.T) {
	t.Setenv("VIL_MODE", "")
	if ModeFromEnv() != ModeSidecar {
		t.Fatalf("ModeFromEnv default = %q, want %q", ModeFromEnv(), ModeSidecar)
	}
	t.Setenv("VIL_MODE", "wasm")
	if ModeFromEnv() != ModeWasm {
		t.Fatalf("ModeFromEnv wasm = %q", ModeFromEnv())
	}
}

// TestPipelineManifestStructure exercises sink/source/route/compile surface.
func TestPipelineManifestStructure(t *testing.T) {
	p := NewPipeline("test-pipe", 3080).
		Sink(SinkOpts{Port: 3080, Path: "/trigger"}).
		Source(SourceOpts{URL: "http://localhost:4545/v1/chat", Format: "sse"}).
		Route("http_sink", "http_source", "LoanWrite")

	y := p.ToYaml()
	mustContain(t, y, []string{
		`vil_version: "6.0.0"`,
		"name: test-pipe",
		"port: 3080",
		"token: shm",
		"nodes:",
		"http_sink",
		"http_source",
		`url: "http://localhost:4545/v1/chat"`,
		"format: sse",
		"routes:",
		"from: http_sink",
		"to: http_source",
		"mode: LoanWrite",
	})
}

// TestServerManifestStructure exercises service/endpoint/compile surface.
func TestServerManifestStructure(t *testing.T) {
	gw := NewService("gw")
	gw.Endpoint("GET", "/health", "health")
	gw.Endpoint("POST", "/echo", "echo")

	s := NewServer("vil-test-server", 8080).Service(gw)
	y := s.ToYaml()
	mustContain(t, y, []string{
		`vil_version: "6.0.0"`,
		"name: vil-test-server",
		"port: 8080",
		"mode: server",
		"services:",
		"- name: gw",
		"prefix: /api/gw",
		"endpoints:",
		"method: GET",
		"path: /health",
		"handler: health",
		"method: POST",
		"path: /echo",
		"handler: echo",
	})
}
