package discovery

import "github.com/gamelink-project/gamelink/shim/rustcore"

// GameType represents the default game mode of a world.
const (
	GameTypeSurvival int32 = iota
	GameTypeCreative
	GameTypeAdventure
)

// ServerData defines the binary structure representing worlds in Minecraft: Bedrock Edition.
// It is encapsulated in [ResponsePacket.ApplicationData] and sent in response to [RequestPacket]
// broadcasted by clients on port 7551.
type ServerData struct {
	// ServerName is the name of the server. It is typically the player name of the owner
	// hosting the server and is displayed below the LevelName in the world card.
	ServerName string
	// Protocol is the protocol version used by the host of the server.
	Protocol int32
	// Version is the game version used by the host of the server.
	Version string
	// LevelName identifies the name of the world and appears at the top of ServerName in the world card.
	LevelName string
	// GameType is the default game mode of the world. Players receive this game mode when they
	// join. It remains unchanged during gameplay and may be updated the next time the world is hosted.
	GameType int32
	// PlayerCount is the amount of players currently connected to the world.
	PlayerCount int32
	// MaxPlayerCount is the maximum amount of players allowed to join the world.
	MaxPlayerCount int32
	// EditorWorld is a value dictates if the world was created as a project in Editor Mode.
	// When enabled, the server or world card is only visible to clients in Editor Mode.
	EditorWorld bool
	// Hardcore indicates that the world is in hardcore mode. When enabled, it is common to also set
	// GameType to Survival (0) as well to reproduce expected behavior.
	Hardcore bool
	// AcceptsOnlineAuth indicates whether the server accepts online-authenticated (Xbox Live) players.
	// When set to true, the client prefers to use a multiplayer token issued by the authorization service.
	//
	// If the client is not logged in or the authorization service is unable to issue a multiplayer token
	// and AcceptsSelfSignedAuth is false, the client will fast-fail before connecting to the server. Otherwise,
	// it falls back to using a self-signed JWT token in the Login packet.
	//
	// When both AcceptsOnlineAuth and AcceptsSelfSignedAuth are false, clients are unable to join this server.
	AcceptsOnlineAuth bool
	// AcceptsSelfSignedAuth indicates whether the server accepts self-signed (LAN) authentication.
	// When any of the following conditions are true, clients will use a self-signed JWT token in the
	// Login packet:
	//   - The client is not logged in to an Xbox Live account, or the authorization service is unable to issue a multiplayer token due to service errors.
	//   - AcceptsOnlineAuth is false and AcceptsSelfSignedAuth is true.
	// When AcceptsOnlineAuth is true, the client prefers to use a multiplayer token issued by the authorization service.
	// When both AcceptsOnlineAuth and AcceptsSelfSignedAuth are false, clients are unable to join this server.
	AcceptsSelfSignedAuth bool
	// Nonce is a randomly generated, hex-encoded string produced by the host. Clients connecting
	// to this server must include this same value in the 'Nonce' field of the ClientData sent
	// in the connection request of the Login packet.
	Nonce string
	// ConnectionType indicates the connection type used alongside the transport layer.
	// In vanilla, this is typically 4 for using LAN as a signaling for NetherNet.
	// Other values are supported but are currently not useful in LAN discovery.
	ConnectionType int32
}

// Server advertisement strings are byte strings on the wire. []byte fields
// preserve them through JSON/base64, including non-UTF-8 host advertisements.
type serverDataWire struct {
	ServerName            []byte
	Protocol              int32
	Version               []byte
	LevelName             []byte
	GameType              int32
	PlayerCount           int32
	MaxPlayerCount        int32
	EditorWorld           bool
	Hardcore              bool
	AcceptsOnlineAuth     bool
	AcceptsSelfSignedAuth bool
	Nonce                 []byte
	ConnectionType        int32
}

func (d *ServerData) MarshalBinary() ([]byte, error) {
	var out struct {
		Data []byte `json:"data"`
	}
	err := rustcore.Invoke("discovery.ad_encode", serverDataWire{
		[]byte(d.ServerName), d.Protocol, []byte(d.Version), []byte(d.LevelName), d.GameType, d.PlayerCount, d.MaxPlayerCount, d.EditorWorld, d.Hardcore, d.AcceptsOnlineAuth, d.AcceptsSelfSignedAuth, []byte(d.Nonce), d.ConnectionType}, &out)
	return out.Data, err
}
func (d *ServerData) UnmarshalBinary(data []byte) error {
	var v serverDataWire
	if err := rustcore.Invoke("discovery.ad_decode", struct {
		Data []byte `json:"data"`
	}{nonNil(data)}, &v); err != nil {
		return err
	}
	*d = ServerData{ServerName: string(v.ServerName), Protocol: v.Protocol, Version: string(v.Version), LevelName: string(v.LevelName), GameType: v.GameType, PlayerCount: v.PlayerCount, MaxPlayerCount: v.MaxPlayerCount, EditorWorld: v.EditorWorld, Hardcore: v.Hardcore, AcceptsOnlineAuth: v.AcceptsOnlineAuth, AcceptsSelfSignedAuth: v.AcceptsSelfSignedAuth, Nonce: string(v.Nonce), ConnectionType: v.ConnectionType}
	return nil
}
