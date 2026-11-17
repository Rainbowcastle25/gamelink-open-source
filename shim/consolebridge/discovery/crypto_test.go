// AES-ECB wire encoding is required by NetherNet LAN discovery and does not
// provide peer authentication. Discovery addresses are separately constrained.
package discovery

import (
	"crypto/aes"
	"crypto/sha256"
	"encoding/binary"
	"errors"
)

var key = sha256.Sum256(binary.LittleEndian.AppendUint64(nil, 0xdeadbeef))

func encrypt(src []byte) []byte {
	b, _ := aes.NewCipher(key[:])
	padding := b.BlockSize() - len(src)%b.BlockSize()
	dst := make([]byte, len(src)+padding)
	copy(dst, src)
	for i := len(src); i < len(dst); i++ {
		dst[i] = byte(padding)
	}
	for i := 0; i < len(dst); i += b.BlockSize() {
		b.Encrypt(dst[i:], dst[i:])
	}
	return dst
}
func decrypt(src []byte) ([]byte, error) {
	b, _ := aes.NewCipher(key[:])
	if len(src) == 0 || len(src)%b.BlockSize() != 0 {
		return nil, errors.New("invalid ciphertext length")
	}
	dst := make([]byte, len(src))
	for i := 0; i < len(dst); i += b.BlockSize() {
		b.Decrypt(dst[i:], src[i:])
	}
	n := int(dst[len(dst)-1])
	if n == 0 || n > b.BlockSize() {
		return nil, errors.New("invalid padding")
	}
	for _, v := range dst[len(dst)-n:] {
		if int(v) != n {
			return nil, errors.New("invalid padding")
		}
	}
	return dst[:len(dst)-n], nil
}
