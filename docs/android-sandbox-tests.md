# Android sandbox regression tests

`NativeSandboxTest` exercises the Android app process boundary. It checks
Keystore-backed `SecretStore` encryption, the JNI lifecycle reducer, and Go
identity/invitation validation. It zeroes its test private-key byte array and
asserts malformed invitations are not echoed in errors.

The storage test creates
`targetContext.cacheDir/secret-store-test-<UUID>` and overrides
`getNoBackupFilesDir()` to use that isolated directory while using the target
app's Android Keystore identity. It never opens or deletes the application's
real `noBackupFilesDir/secrets-v1` file.

## Go DNS/TLS probe

The DNS/TLS probe is opt-in because it needs internet access:

```sh
cd apps/android
./gradlew connectedDebugAndroidTest \
  -Pandroid.testInstrumentationRunnerArguments.network=true
```

`mobile.SandboxDiagnostics` checks only the fixed public Mojang Bedrock
protocol guide over HTTPS from Go. It reports DNS/TLS booleans, interface
count and Go version; it does not return IP addresses, HTTP content,
credentials or raw errors. The probe follows no redirects and has a ten-second
deadline. On Android API 37+, the instrumentation test grants
`ACCESS_LOCAL_NETWORK`; this permission is not needed for ordinary internet
access.

On the Android API 37 16 KiB emulator, the five sandbox instrumentation tests
passed, including Keystore storage, JNI reducer, Go identity/error redaction,
Go DNS/TLS, and the local Tailcat fixture reaching discovery and stopping
cleanly with UDP 7551 released. The fixture's first ready status reported
`path: unknown`; this run does not establish direct-versus-DERP selection.
The API 37 local-network permission was granted by instrumentation, so the
interactive permission prompt itself was not exercised. These results do not
prove a real BDS media session, firewall/NAT behavior, console compatibility or
gameplay.

## Opt-in local Tailcat fixture

`TestAndroidTailcatFixture` starts a hermetic relay, authenticated bridge host
and synthetic discovery responder. Its fixture JSON contains a private key and
invitation. Keep the fixture in a mode-0700 temporary directory with a
mode-0600 file. Never print it, paste it into chat, or attach it to a report.
The fixture explicitly sets `allowLocalRelay: true` only for this
instrumentation run. Production Android configuration leaves local relay
access false.

Terminal 1, from the repository root, starts the test host and waits up to
three minutes:

```sh
umask 077
fixture_dir="$(mktemp -d)"
chmod 700 "$fixture_dir"
fixture="$fixture_dir/android-fixture.json"
GAMELINK_ANDROID_FIXTURE="$fixture" go test -v -count=1 \
  -run '^TestAndroidTailcatFixture$' ./tests/integration
```

Terminal 2, after the host test reports the fixture is ready, copies the JSON
directly to the target app's private files directory over stdin and runs the
instrumentation test. The fixture bytes are not printed:

```sh
adb shell 'run-as io.gamelink.app sh -c "mkdir -p files; umask 077; cat > files/android-fixture.json"' < "$fixture"
cd apps/android
./gradlew connectedDebugAndroidTest \
  -Pandroid.testInstrumentationRunnerArguments.fixture=true \
  -Pandroid.testInstrumentationRunnerArguments.class=io.gamelink.app.NativeSandboxTest#actualTailcatFixtureConnectsThroughGoSandboxAndReleasesDiscoveryPort
```

After the instrumentation result returns, signal Terminal 1 to stop and remove
its host fixture. The Android emulator cannot create a file on the host:

```sh
touch "$fixture.stop"
```

The instrumentation test deletes its app-side fixture in a `finally` block,
stops the Go runtime, and checks that UDP port 7551 can be rebound. The fixture
currently covers Tailcat plus discovery connectivity and cleanup; it does not
complete a real BDS media session or establish console compatibility. The
test can fail if the emulator lacks an active private IPv4 interface or local
DERP reachability.
