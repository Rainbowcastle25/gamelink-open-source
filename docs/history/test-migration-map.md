# Test migration map

This records where each test went when the legacy Go hosts under
`tests/testsupport/` (a Go `Supervisor`, `AgentStore`/`ClientStore` and desktop
`client.Manager` that duplicated the Rust agent and desktop) were deleted, so
the end-to-end tests exercise the code that actually ships.

## Unit tests

Every `Test*` function in the legacy Go packages under `tests/testsupport/` and
where its assertions live now. The Go wrappers only forwarded to the Rust core
through `rustcore.Invoke`, or (for `supervisor_test.go` and
`client/lifecycle_test.go`) tested orchestration that now lives in the Rust
hosts. Paths below are relative to the crate source directory.

Crate shorthand:

- `core/` is `core/crates/gamelink-core/src/`
- `agent/` is `core/crates/gamelink-agent/src/`
- `desktop/` is `apps/desktop-tauri/src/`

Test-only bridge seam: `agent/bridge.rs` and `desktop/bridge.rs` each contain a
`#[cfg(test)] mod fake` that installs a per-thread stand-in for the Go archive.
It is compiled out of production builds. The tests in `agent/runtime.rs` and
`desktop/host.rs` use it with a strict script (an unscripted bridge method
panics), and record the ordered `join`/`check`/`start`/`stop`/`shutdown` calls.
`desktop/host.rs` also gained a private `App::load_in(dir)` (the old `load()`
minus its environment lookup) so tests need not touch `GAMELINK_HOME`.

Counts: 42 `Test*` functions: 37 ported, 5 dropped. (Plus one benchmark,
`BenchmarkStableReconcile`, dropped, and the non-compiled
`core/fixtures/lifecycle-freeze_original_go_test.go.txt`, whose oracle is the recorded
`lifecycle-go-6c89f64.json` fixture already read by
`core/client_lifecycle.rs::frozen_go_policy_and_transition_fixtures`.)

### client/launch_windows_test.go

| Go test | Rust test |
|---|---|
| `TestLaunchPassesArguments` | `core/launch.rs` `dispatch_tests::shell_execute_passes_arguments_to_a_batch_file` (`#[cfg(windows)]`, runs in the `windows-rust` CI job); non-Windows spawn and argument splitting: `launch::tests::whitespace_arguments_and_working_directory`, `dispatch_tests::program_launch_succeeds_and_passes_no_arguments_for_empty_args` |

### client/lifecycle_test.go

The Go tests drove a goroutine-based `Manager` with fake tunnels and channel
barriers. The Rust host runs each command under one lock and executes `stop`
effects to completion inside that command, so the barriers cannot be observed;
the ordering they protected is asserted through the recorded bridge calls.

| Go test | Rust test |
|---|---|
| `TestJoinMarksPendingAndAwaitsProbeClientClosure` | `desktop/host.rs` `join_passes_the_unsealed_address_and_owns_the_generation` (pending display, early connected status ignored, duplicate join refused), `check_while_connected_answers_from_the_live_status_and_join_cancels_a_probe` (probe stopped before the join binds, connected shortcut); `core/client_lifecycle.rs` `early_status_connected_shortcut_and_cancelled_checks`, `probe_takeover_restores_check_and_stale_callbacks_are_inert` |
| `TestStaleJoinSuccessClosesBeforeReplacementBinds` | `desktop/host.rs` `stale_generation_callbacks_cannot_change_a_newer_join`, `terminal_events_are_applied_after_statuses_regardless_of_arrival_order`; `core/client_lifecycle.rs` `probe_takeover_restores_check_and_stale_callbacks_are_inert` (stale `join_end` yields `close_returned`) |
| `TestRetryRetainsBusyPortGuardUntilTunnelClosed` | `desktop/host.rs` `retry_stops_the_predecessor_first_and_the_port_guard_persists_while_unreachable`, `local_port_can_change_when_idle_and_renames_are_always_allowed`; `core/client_lifecycle.rs` `retry_and_busy_ownership_are_independent_of_display`, `local_port_edits_are_refused_while_a_resource_is_alive` |
| `TestCheckDuplicateCancellationRestoreAndRemoveWait` | `desktop/host.rs` `check_probes_the_server_and_releases_the_probe_when_it_ends`, `check_outcomes_error_and_cancelled`, `remove_server_restores_the_lifecycle_when_the_write_fails_and_logs_on_success`; `core/client_lifecycle.rs` `check_outcomes_and_removal_semantics` |
| `TestCloseAwaitsProbesAndCancelsDemoTimers` | `desktop/host.rs` `shutdown_closes_every_resource_then_shuts_the_bridge_down`, `shutdown_reports_failures_but_still_shuts_the_bridge_down`; `core/client_lifecycle.rs` `stale_demo_timer_and_close_do_not_reanimate` |
| `TestDemoReplacementRejectsOldTimer` | `desktop/host.rs` `replacing_a_demo_join_discards_the_old_timer`, `demo_servers_simulate_a_connection_without_touching_the_network`; `core/client_lifecycle.rs` `demo_joins_require_demo_mode_and_a_known_server` |
| `TestJoinFailurePreservesFriendlyError` | `desktop/host.rs` `failed_join_reports_a_friendly_error_and_releases_the_resource`, `no_response_is_reported_as_unreachable_and_is_a_warning`; `core/client_lifecycle.rs` `join_failures_keep_a_friendly_message_and_distinguish_no_response` |
| `TestDefaultJoinAdapterPreservesNilTunnelOnError` | dropped: tested the Go `transport.Join` adapter (`m.host.join`), which no longer exists; the Go side of that contract is `shim/desktop/main_test.go` |
| `TestCheckAwaitsRetiringTunnelAfterDisconnect` | `desktop/host.rs` `check_after_disconnect_starts_only_after_the_tunnel_was_stopped` |
| `TestEarlyStatusNotificationCanDisconnectReentrantly` | dropped: tested re-entrancy of the Go `Emit` callback into `Manager.Disconnect`. Go callbacks now only enqueue events that `App::pump` applies under the host lock, so re-entrancy cannot occur by construction. The early-status ordering is covered by `join_passes_the_unsealed_address_and_owns_the_generation` |
| `TestInitialJoinActivityNotificationCanCloseReentrantly` | dropped: same reason (re-entrant `Close` from an activity notification) |

### config/config_test.go

| Go test | Rust test |
|---|---|
| `TestClientStoreReadWritePersist` | `core/config.rs` `behaviour::client_store_read_write_persist`; end to end through the host: `desktop/host.rs` `add_server_validates_the_address_seals_it_and_queues_a_check`, `update_settings_validates_persists_and_announces` |
| `TestClientStoreRejectsInvalid` | `core/config.rs` `behaviour::client_store_rejects_invalid_servers`, `client_mutations_report_missing_servers_limits_and_unknown_actions`, `client_settings_validation` |
| `TestClientStoreCorruptFileIsNotOverwritten` | `core/config.rs` `behaviour::corrupt_client_files_are_rejected_and_left_untouched` |
| `TestAgentStoreLifecycle` | `core/config.rs` `behaviour::agent_store_lifecycle`. Dropped assertions: server identity equality across stores and per-game key inequality compared Go `tailcat` key objects; Rust persists the keys it is given, which the test asserts via `keys_load` |
| `TestAgentStoreRejectsInvalid` | `core/config.rs` `behaviour::agent_store_rejects_invalid_games_and_keys`, `agent_limits_and_duplicate_ids`, `game_prepare_applies_defaults_and_normalises_destinations` |
| `TestConsoleBridgePortRange` | `core/config.rs` `behaviour::console_bridge_validation` |

### config/example_test.go, config/slug_compat_test.go

| Go test | Rust test |
|---|---|
| `TestExampleAgentConfigLoads` | `core/config.rs` `behaviour::shipped_example_agent_config_loads_and_has_no_key_material` (reads `config/examples/agent.example.json` via `include_str!`) |
| `TestAddGameSimpleLowercaseSlugsRemainDistinct` | `core/config.rs` `behaviour::slugs_use_go_simple_lowercase_and_stay_distinct` (plus the recorded Go slug fixtures in `tests::original_go_fixtures`) |

### config/rust_storage_test.go

| Go test | Rust test |
|---|---|
| `TestRustLargeAgentSnapshotAndConcurrentMutations` | `core/config.rs` `behaviour::large_agent_snapshot_and_concurrent_mutations` |
| `TestRustClientRejectsTypedCorruptionWithoutOverwrite` | `core/config.rs` `behaviour::corrupt_client_files_are_rejected_and_left_untouched` |
| `TestRustClientWriteFailurePreservesMemory` | `core/config.rs` `behaviour::client_write_failure_returns_an_error_and_keeps_the_input_config`; the in-memory half (`App.cfg` unchanged) is `desktop/host.rs` `a_failed_config_write_leaves_the_in_memory_configuration_unchanged` |
| `TestRustAgentCorruptionCannotBeMutated` | `core/config.rs` `behaviour::duplicate_member_corruption_cannot_be_hidden_or_mutated` |
| `TestRustAgentSnapshotWireOrder` | `core/config.rs` `behaviour::agent_snapshots_follow_go_wire_order_semantics` (expectations written from `encoding/json` merge rules; they passed unchanged against the Rust decoder that these Go tests already validated), plus existing `tests::ordered_typed_decode_matches_go_merge_and_null_rules` |
| `TestRustAgentDuplicateCorruptionCannotBeHiddenOrMutated` | `core/config.rs` `behaviour::duplicate_member_corruption_cannot_be_hidden_or_mutated` |
| `TestRustClientSettingsWireOrder` | `core/config.rs` `behaviour::client_settings_follow_go_wire_order_semantics` |

### identity/identity_test.go

| Go test | Rust test |
|---|---|
| `TestIdentityPersistsAcrossRestart` | `core/storage.rs` `tests::identity_save_then_load_round_trips_and_is_sealed`; host: `desktop/host.rs` `first_launch_creates_and_seals_the_device_identity_then_reuses_it` |
| `TestIdentityFileDoesNotContainPlaintextPrivateKey` | `core/storage.rs` `tests::identity_save_then_load_round_trips_and_is_sealed` (envelope), `tests::dpapi::identity_file_stores_a_dpapi_protected_key` (`#[cfg(windows)]`, plaintext absent) |
| `TestIdentityCorruptFileIsAnErrorNotRegenerated` | `core/storage.rs` `tests::identity_load_rejects_corrupt_files_without_replacing_them`, `identity_load_rejects_wrong_version_and_unopenable_keys` |
| `TestParsePublicKey` | existing `core/pure.rs` `tests::go_fixtures` (recorded Go outputs for `identity.public_key`), plus bad-key cases in `core/config.rs` `behaviour::agent_store_rejects_invalid_games_and_keys` |

### security/security_test.go

| Go test | Rust test |
|---|---|
| `TestWriteFileAtomic` | `core/storage.rs` `tests::write_atomic_replaces_creates_parents_and_leaves_no_temp_files`, `write_atomic_reports_unreplaceable_targets_and_bad_parents` |
| `TestWriteFileAtomicConcurrentWritersNeverTear` | `core/storage.rs` `tests::concurrent_writers_never_tear_the_file` (per-write errors ignored, as on Windows replacement can race) |
| `TestSealRoundTrip` | `core/storage.rs` `tests::seal_round_trips_and_uses_the_gls1_envelope`, `unseal_rejects_malformed_envelopes`, `dispatch_seal_and_unseal`; Windows DPAPI: `tests::dpapi::seal_round_trips_through_dpapi`, `sealed_payload_is_encrypted_not_plaintext_base64`, `a_plain_non_windows_envelope_is_rejected_on_windows`, `tampered_blob_fails_to_unprotect`, `line_wrapped_envelope_still_opens` |
| `TestValidators` | existing `core/pure.rs` `tests::go_fixtures` (recorded Go outputs for port, slug, destination, public-IP, hostname and display-text validators) |
| `TestRedactor` | existing `core/pure.rs` `tests::go_fixtures` (`security.redact`); host-side redaction is asserted by `desktop/host.rs` `activity_log_limit_clear_and_redaction`; `agent/runtime.rs` `log_effects_do_not_call_the_bridge` only checks that log effects stay local |

### supervisor_test.go

| Go test | Rust test |
|---|---|
| `TestReconcileWritesRuntimeOnlyWhenStateChanges` | `agent/runtime.rs` `tests::unchanged_reconcile_does_not_rewrite_runtime_json` (byte-equal file and no extra `stats` publish) |
| `TestInvalidConfigStopsServingState` | `agent/runtime.rs` `tests::invalid_config_stops_all_games_and_recovers_when_fixed`, `a_missing_agent_file_fails_closed_the_same_way`. Behaviour difference, intentional: the Go `Reconcile()` returned the configuration error; the Rust loop logs it, stops every game, publishes `state: error` with `configuration is invalid: ...` in `runtime.json`, and keeps polling so a fixed file recovers |
| `TestSupervisorBridgeChangeRequestsRestart` | `core/supervisor.rs` `tests::console_bridge_change_stops_before_it_restarts`, `key_file_change_restarts_and_discovery_port_is_part_of_the_bridge`; host: `agent/runtime.rs` `revocation_and_key_changes_stop_before_they_restart` |
| `TestSupervisorLogCanReadSnapshot` | dropped: tested that Go `Logf` callbacks could call `Snapshot()` without deadlocking the state mutex. The Rust `Runtime` has no callbacks or locks (`&mut self`, effects return values); removal logging is covered by `agent/runtime.rs` `removed_games_are_stopped_and_dropped_from_runtime_json` |
| `TestConcurrentSupervisorReconcile` | dropped: tested Go mutex safety under concurrent `Reconcile`. `Runtime` is single-owner and `serve` is one loop; concurrent writers of the config files are covered by `core/config.rs` `behaviour::large_agent_snapshot_and_concurrent_mutations` |
| `BenchmarkStableReconcile` (benchmark) | dropped: its correctness check (no rewrite when stable) is `unchanged_reconcile_does_not_rewrite_runtime_json` |

### New tests for effect execution without a Go counterpart

`agent/runtime.rs`: `first_reconcile_starts_the_game_and_publishes_runtime_json`
(start request content, devices stripped), `publish_merges_live_stats_into_the_game_rows`,
`no_approved_devices_stops_the_game_and_reports_inactive`,
`duplicate_game_ids_abort_the_reconcile`,
`failed_start_is_reported_and_retried_only_after_the_cooldown` (StartCompleted
error path), `start_errors_before_the_bridge_are_reported_without_calling_it`,
`an_invalid_approved_device_blocks_the_game`,
`a_stale_successful_start_is_stopped_at_its_exact_generation`,
`bridge_failures_propagate_from_effects`, and three `serve` tests (clean stop
removes `runtime.json` after `stop` then `shutdown`; cleanup on a failing loop;
failed shutdown reported).

`desktop/host.rs`: identity load/creation, activity trimming, join request
contents, unreadable addresses, failed and stale joins, retry ordering, port
guard, check scheduling (`checkAll` one at a time, startup delay), demo timers,
shutdown ordering, bridge log rotation, invitations, `launchGame`, diagnostics.
A join the bridge rejects outright now ends in `error` and can be retried
(`failed_bridge_join_shows_an_error_and_can_be_retried`).

## Integration tests

How they run: `tests/integration` starts a local DERP/STUN relay in the test
process and drives the **shipped Rust agent binary** (`GAMELINK_AGENT_BINARY`,
built with `cargo build -p gamelink-agent`) as a subprocess through its CLI
(`init`, `games add`, `devices approve|revoke`, `invite`, `serve`). The
only fixture injection is pointing a game's pinned relay at the local DERP,
because the CLI deliberately has no insecure or testing flags. Clients are the
real Go transport (`shim/transport`) with the options the shipped desktop host
sends (`desktopOpts`). A missing `GAMELINK_AGENT_BINARY` is a hard failure when
`CI` is set and a skip otherwise.

Helper API (`helpers_test.go`): `newAgentEnv`, `addGame`, `approve`, `revoke`,
`serve`/`stop` (SIGTERM, kill fallback), `addr` (waits for runtime state
`serving` with every approved device, then returns the address printed by the
agent's own `invite` command), `keys` (address derived independently in Go),
`isServing`, `waitState`, `statsAfter`.

| Test | Now drives |
|---|---|
| `TestTCPEchoThroughTailcat` | Rust agent (`serve`) + transport `Join`, 8 concurrent 8 KB echoes |
| `TestTCPHalfClosePreserved` | Rust agent + transport `Join` |
| `TestUnauthorizedKeyCannotConnectEvenWithAddress` | Rust agent + transport and raw Tailcat client; accepted-session count now read from the agent's published `runtime.json` stats (waits for a publication newer than the attempts) |
| `TestEmptyAllowlistNeverExposesGame` | `transport.StartGameServer` refusal (unchanged) + Rust agent keeps the game `inactive` and unreachable (now also asserted via runtime state, not a Go supervisor) |
| `TestRevokedDeviceLosesAccess` | Rust agent; revoke through the real CLI while a session is open |
| `TestRevokingLastDeviceStopsServing` | Rust agent; runtime state goes `inactive` |
| `TestServerIsNotAnExitNodeAndServesOnlyItsPort` | Rust agent + raw Tailcat client |
| `TestPeerAddrMatchesTailcat` | Tailcat server directly (no agent; unchanged) |
| `TestOfflineAddressMatchesLiveServer` | Address derived in Go from the saved key file must equal the address the shipped `invite` prints, and a client joins with it |
| `TestUDPEchoPreservesDatagramBoundaries`, `TestUDPMultipleLocalEndpointsGetOwnReplies`, `TestUDPOversizeIsDroppedAndReportedNotTruncated`, `TestUDPUnauthorizedKeyCannotConnect` | Rust agent (Factorio UDP game) + transport `Join` |
| `TestLocalPortConflicts`, `TestPreferredPortUsedWhenFree`, `TestDisconnectReleasesResources`, `TestUDPDisconnectReleasesPort` | Rust agent + transport `Join` (client-side resource behaviour; the goroutine baseline now covers only the client side, the agent is a separate process) |
| `TestClientReconnectsAfterServerRestart` | Rust agent stopped with SIGTERM and started again; the same tunnel must reconnect |
| `TestRelayOnlyFallbackWorksAndIsReportedAsRelayed` | Rust agent + client with `TS_DEBUG_ALWAYS_USE_DERP` |
| `TestInvalidConfigFailsClosed` | Rust agent: an approved device joins, `agent.json` is corrupted, the game must reach state `error` and the same device can no longer join |
| `TestRustAgentPterodactylManualRefresh` | Rust agent + local fake Panel (added in #9; now uses the shared `agentBinary` lookup, so it fails instead of skipping in CI) |
| `TestRustAgentSubprocessRevocationAndFailClosed` | Rust agent subprocess (unchanged scenario; now shares the binary lookup and CLI helpers) |
| `TestDesktopJourneyInviteCheckJoinDisconnectRejoin` | Rust agent CLI invitation (`invite.Parse`) -> `transport.CheckWithRelayPolicy` (shim `check`) -> `transport.Join` with the desktop's options -> Stop frees the port -> rejoin with the same identity |
| `TestDesktopUnapprovedDeviceIsUnreachable` | Rust agent + `Join`/`CheckWithRelayPolicy` with an unapproved key: `ErrNoResponse`, message mentions approval, no tunnel/local address |
| `TestDesktopRetryAfterConnectionLost` | Rust agent stopped; `AutoReconnect=false` tunnel reaches `error` and releases its local port; new `Join` after the agent returns |
| `TestDesktopRetryWhileAutoReconnecting` | Rust agent stopped; `AutoReconnect=true` tunnel reaches `unreachable`; a retry fails honestly with `ErrNoResponse` while down and succeeds after the agent returns |
| `TestJoinReportsActualLocalPortWhenStandardPortIsBusy` | Rust agent + `Join` with the standard UDP port occupied: fallback port reported and used (replaces the port half of `TestLaunchFactorioConnectsToActualLocalPort`) |
| `TestConsoleBridgeTailcatMediaAndSessionIsolation`, `TestConsoleBridgeRejectsUnapprovedPeer` | `transport.StartGameServer` + `consolebridge` directly (unchanged; they never used the legacy host). Not run through the Rust agent: the CLI cannot yet start a bridge on loopback addresses in a hermetic test |
| `TestPublicRelayEndToEnd` (build tag `external`) | Rust agent with the **real** public relay (no region patching, strict invitation validation) + transport `Join` over the internet |
| `TestResourceProfile` (opt-in `GAMELINK_RESOURCE_PROFILE=1`) | Rust agent child + clients; every phase samples the test process and the agent process separately |
| `TestAndroidTailcatFixture` (opt-in) | `transport.StartGameServer` + `consolebridge` (unchanged) |

### Integration tests deleted

| Test | Reason |
|---|---|
| `TestDesktopRejectsConsoleBridgeInviteClearly` | Dropped: the rejection ("requires the Android Console Bridge") is Rust (`BRIDGE` in `core/.../desktop.rs`) and was asserted only through the deleted Go `Manager`. Now covered by `core/desktop.rs` `preview_rejects_bridge_and_never_returns_full_address`. |
| `TestDemoModeIsLabeledAndOptIn` | Dropped: demo mode never touched the network; covered by `desktop/host.rs` `demo_servers_simulate_a_connection_without_touching_the_network`. |
| `TestSettingsIgnoreUnchangedStalePaths` | Dropped: settings validation is Rust-owned; covered by `core/desktop.rs` `settings_validate_only_changed_paths_and_preserve_empty_override`. |
| `TestLaunchArgsSettingsRoundTrip` | Dropped: launch-argument templates and settings persistence are Rust-owned. |
| `TestLaunchFactorioConnectsToActualLocalPort` (launch args half) | Dropped: building `--mp-connect <addr>` is Rust-owned; the port/transport half is kept as `TestJoinReportsActualLocalPortWhenStandardPortIsBusy`. |
| `TestLaunchJavaArgsAndCustomTemplate`, `TestLaunchBedrockUsesURI` | Dropped: pure argument/URI formatting in the Rust desktop; no transport assertion; covered by `core/desktop.rs` `launch_plan_variants` and the `desktop/host.rs` launch tests. |

## Fixture moved

The frozen Go lifecycle recording `lifecycle-go-6c89f64.json` (with its README and
generator) moved from `tests/testsupport/client/testdata/` to `core/fixtures/`,
where `client_lifecycle.rs` loads it with `include_str!`. Its bytes are unchanged.
