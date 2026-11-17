// Package games is the registry of supported game types: their transport
// protocol, default port and player-facing connection instructions.
package games

import "github.com/gamelink-project/gamelink/shim/rustcore"

// Protocol is the transport protocol a game uses.
type Protocol string

const (
	TCP Protocol = "tcp"
	UDP Protocol = "udp"
)

// Valid reports whether p is a supported protocol.
func (p Protocol) Valid() bool {
	var valid bool
	return rustcore.Invoke("games.valid_protocol", map[string]any{"proto": p}, &valid) == nil && valid
}

// Type identifies a kind of game.
type Type string

const (
	MinecraftJava    Type = "minecraft-java"
	MinecraftBedrock Type = "minecraft-bedrock"
	Factorio         Type = "factorio"
	Custom           Type = "custom"
)

// Info describes a game type.
type Info struct {
	Type        Type     `json:"type"`
	Name        string   `json:"name"`
	Protocol    Protocol `json:"protocol"` // empty for Custom (either)
	DefaultPort uint16   `json:"defaultPort"`
	// Verified says how far this alpha was able to test the game: "real"
	// means a real game client/server was used, "protocol" means only
	// generic TCP/UDP forwarding was tested.
	Verified string `json:"verified"`
	// Hint is shown next to the local address to explain how to connect.
	Hint string `json:"hint"`
	// LaunchArgs is the default command-line template passed to the game's
	// program so it connects straight to the server. Empty when the usual
	// program has no such option. See ExpandLaunch for the placeholders.
	LaunchArgs string `json:"launchArgs"`
	// LaunchURI, when set, opens the game and connects without a configured
	// program (Windows only). Same placeholders as LaunchArgs.
	LaunchURI string `json:"launchUri"`
}

// LaunchHost is the address games connect to: GameLink's loopback listener.
// Launch arguments only ever contain this, never the server's real address.
const LaunchHost = "127.0.0.1"

// ExpandLaunch fills a launch template: {host} becomes 127.0.0.1, {port} the
// local port and {address} both as host:port.
func ExpandLaunch(tmpl string, port int) string {
	var out string
	_ = rustcore.Invoke("games.expand", map[string]any{"text": tmpl, "port": port}, &out)
	return out
}

func HasPlaceholder(tmpl string) bool {
	var out bool
	return rustcore.Invoke("games.placeholder", map[string]any{"text": tmpl}, &out) == nil && out
}

func All() []Info {
	var out []Info
	_ = rustcore.Invoke("games.all", map[string]any{}, &out)
	return out
}

func Lookup(t Type) (Info, bool) {
	for _, info := range All() {
		if info.Type == t {
			return info, true
		}
	}
	return Info{}, false
}

func CheckProtocol(t Type, proto Protocol) error {
	return rustcore.Invoke("games.check", map[string]any{"game": t, "proto": proto}, nil)
}
