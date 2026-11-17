package purefixtures

import (
	"encoding/json"
	"github.com/gamelink-project/gamelink/shim/games"
	"github.com/gamelink-project/gamelink/shim/identity"
	"github.com/gamelink-project/gamelink/shim/invite"
	"github.com/gamelink-project/gamelink/shim/security"
	"net/netip"
	"os"
	"reflect"
	"testing"
)

// Frozen before replacing the Go implementations; Rust's unit test reads the
// exact same file. These exercise public Go adapters as well as the C ABI.
func TestOriginalGoFixtures(t *testing.T) {
	data, err := os.ReadFile("../../core/fixtures/pure.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixtures []struct {
		Method string
		Input  map[string]any
		Value  any
		Error  string
	}
	if err := json.Unmarshal(data, &fixtures); err != nil {
		t.Fatal(err)
	}
	for _, f := range fixtures {
		t.Run(f.Method, func(t *testing.T) {
			s := func(k string) string { v, _ := f.Input[k].(string); return v }
			n := func(k string) int { v, _ := f.Input[k].(float64); return int(v) }
			b := func(k string) bool { v, _ := f.Input[k].(bool); return v }
			var value any
			var err error
			switch f.Method {
			case "security.port":
				if b("optional") {
					err = security.ValidateOptionalPort(n("port"))
				} else {
					err = security.ValidatePort(n("port"))
				}
			case "security.slug":
				err = security.ValidateSlug(s("text"))
			case "security.hostname":
				err = security.ValidateHostname(s("text"))
			case "security.display":
				err = security.ValidateDisplayText(s("field"), s("text"), n("maxRunes"), b("required"))
			case "security.destination":
				p, e := security.ParseDestination(s("text"))
				value = p.String()
				err = e
			case "security.public_ip":
				ip, _ := netip.ParseAddr(s("text"))
				value = security.IsPublicIP(ip)
			case "identity.public_key":
				p, e := identity.ParsePublicKey(s("text"))
				value = p.String()
				err = e
			case "games.all":
				value = games.All()
			case "games.check":
				err = games.CheckProtocol(games.Type(s("game")), games.Protocol(s("proto")))
			case "games.expand":
				value = games.ExpandLaunch(s("text"), n("port"))
			case "games.placeholder":
				value = games.HasPlaceholder(s("text"))
			case "security.redact":
				var secrets []string
				for _, secret := range f.Input["secrets"].([]any) {
					secrets = append(secrets, secret.(string))
				}
				value = security.NewRedactor(secrets...).Redact(s("text"))
			case "invite.relay":
				v, e := invite.ValidateAddr(s("fixtureAddr"), invite.Options{AllowLocalRelay: b("allowLocalRelay")})
				value = v
				err = e
			case "invite.encode":
				data, e := json.Marshal(f.Input)
				if e != nil {
					t.Fatal(e)
				}
				var inv invite.Invite
				if e = json.Unmarshal(data, &inv); e != nil {
					t.Fatal(e)
				}
				value, err = invite.Encode(inv, invite.Options{})
			case "invite.decode":
				v, _, e := invite.Parse(s("text"), invite.Options{})
				value = v
				err = e
			default:
				t.Fatalf("unknown fixture method %s", f.Method)
			}
			actualError := ""
			if err != nil {
				actualError = err.Error()
				value = nil
			}
			if actualError != f.Error {
				t.Fatalf("error got %q want %q", actualError, f.Error)
			}
			serialized, e := json.Marshal(value)
			if e != nil {
				t.Fatal(e)
			}
			var normalized any
			if e = json.Unmarshal(serialized, &normalized); e != nil {
				t.Fatal(e)
			}
			if !reflect.DeepEqual(normalized, f.Value) {
				t.Fatalf("value got %s want %+v", serialized, f.Value)
			}
		})
	}
}
