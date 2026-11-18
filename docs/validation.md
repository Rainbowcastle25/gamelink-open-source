# PC real-game validation

## Verified with

Each row is a separate test run. Record the observed path from GameLink. A
passing run requires in-game results for all steps below on that path.

| Game | Client OS | Host type | Path | Date (UTC) | GameLink version | Tester | Result |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Minecraft Java | Windows | Standalone Linux | Direct | — | — | — | not yet verified |
| Minecraft Java | Windows | Standalone Linux | Forced DERP relay | — | — | — | not yet verified |
| Minecraft Java | Windows | Pterodactyl Wings on Linux | Direct | — | — | — | not yet verified |
| Minecraft Java | Windows | Pterodactyl Wings on Linux | Forced DERP relay | — | — | — | not yet verified |
| Minecraft Bedrock (desktop) | Windows | Standalone Linux | Direct | — | — | — | not yet verified |
| Minecraft Bedrock (desktop) | Windows | Standalone Linux | Forced DERP relay | — | — | — | not yet verified |
| Minecraft Bedrock (desktop) | Windows | Pterodactyl Wings on Linux | Direct | — | — | — | not yet verified |
| Minecraft Bedrock (desktop) | Windows | Pterodactyl Wings on Linux | Forced DERP relay | — | — | — | not yet verified |
| Factorio | Windows | Standalone Linux | Direct | — | — | — | not yet verified |
| Factorio | Windows | Standalone Linux | Forced DERP relay | — | — | — | not yet verified |
| Factorio | Windows | Pterodactyl Wings on Linux | Direct | — | — | — | not yet verified |
| Factorio | Windows | Pterodactyl Wings on Linux | Forced DERP relay | — | — | — | not yet verified |

## Set up a test run

1. Use a test host you can reboot and a separate Windows PC. Install the real
   game and a matching dedicated server version. Use a disposable world,
   make sure the test account is admitted by the game's own access controls,
   and check that the game and agent can start after a host reboot. Keep the
   same agent configuration directory throughout.
2. Install the desktop app and Linux agent from the build under test. Turn
   off demo mode. Record the Windows version, host OS or Wings version, game
   client and server versions, desktop build or commit, and agent version
   from `gamelink-agent version`.
3. On standalone Linux, register the test game with the agent using
   [Getting started](getting-started.md). On Wings, register the reachable
   primary gameplay allocation as described in the
   [Pterodactyl guide](pterodactyl.md). For Bedrock on either host, use the
   [desktop NetherNet setup](bedrock-research.md#desktop-manual-server-connections).
   Configure the same UDP media range on BDS and the GameLink game entry,
   and ensure the BDS IPv4 candidate address matches the configured
   destination. On Wings, ensure the agent can reach that address and range.
4. With the agent serving, approve the Windows PC's GameLink public key for
   the test game and share the invitation privately. On the PC, import it
   under **Servers → Add server → Import invitation**. Leave **Local port**
   empty for automatic selection. Make sure the preferred port is initially
   free on the PC.
5. Choose the path for this run.
   - **Direct.** Start the desktop app normally with
     `TS_DEBUG_ALWAYS_USE_DERP` unset. Use a network pair that permits direct
     UDP. After joining, wait for the game card to show **Direct connection**.
   - **Forced DERP relay.** Fully exit GameLink. In a new PowerShell window
     opened in the directory with the app, launch it with the same debug
     setting used by the relay-only integration test:

     ```powershell
     $env:TS_DEBUG_ALWAYS_USE_DERP = 'true'
     Start-Process -FilePath .\GameLink.exe
     Remove-Item Env:TS_DEBUG_ALWAYS_USE_DERP
     ```

     Keep DERP reachable. After joining, wait for the card to show **Relayed**.
     Fully exit that app before running the direct case.

## Join from the game

On the GameLink card, select **Join** (or **Join only** if the card offers
**Play**). Copy the displayed **Local address**. Use the current address each
time, including after reconnects and port fallback.

| Game | Preferred loopback port | Join and check gameplay |
| --- | --- | --- |
| Minecraft Java | TCP 25565 | Open **Multiplayer → Add Server**, enter the complete `127.0.0.1:<port>` address, then select the entry and join. Move, break and place blocks, and verify resulting world changes. |
| Minecraft Bedrock (desktop) | TCP and UDP 19132 | In **Play → Servers → Add Server**, enter `127.0.0.1` as the address and the displayed port in the separate port field. Join the saved entry, move, and break and place blocks in the BDS world. |
| Factorio | UDP 34197 | Open **Multiplayer → Connect to address** and enter the complete `127.0.0.1:<port>` address. Load the test save, move, mine or craft, and place an entity. |

If the Windows Store Bedrock app blocks loopback, check the existing
exemptions with `CheckNetIsolation LoopbackExempt -s` in an elevated prompt.
With permission to change the test PC, add the exemption with
`CheckNetIsolation LoopbackExempt -a -n="Microsoft.MinecraftUWP_8wekyb3d8bbwe"`
and retry. Record whether it was needed. If you added it for the run, remove it
afterward with the same command using `-d` in place of `-a`.

## Run the checks on each path

1. **Join and play for 10 minutes.** Follow the game-specific steps above and
   confirm the game server sees the expected player. Start the timer after
   the world loads. Play actively for 10 minutes, checking that interactions
   continue to reach the server. Record the displayed path at the start and
   end, and any path changes, disconnects or gameplay stalls. If GameLink
   reports another path or **Path not measured**, record the observation.
   Repeat on a suitable network to complete the requested path.
2. **Revoke during gameplay.** While the player is active, run the following
   with the same agent configuration used by the running host. Substitute
   the registered game ID and the Windows PC's public key.

   ```sh
   gamelink-agent devices revoke --game "$GAME_ID" --key "$CLIENT_PUBLIC_KEY"
   gamelink-agent devices list --game "$GAME_ID"
   ```

   Confirm removal from the allowlist. Record when game interactions stop and
   when the game disconnects or times out. Retry from the game, then disconnect
   and try a fresh GameLink Join with the original invitation. Confirm the
   revoked PC cannot resume gameplay or make a fresh connection. Restore
   approval with `gamelink-agent devices approve --game "$GAME_ID" --key "$CLIENT_PUBLIC_KEY" --label "Validation PC"`.
   Join with the existing invitation and confirm gameplay resumes.
3. **Reconnect after a host restart.** While connected and in the game,
   reboot the Linux host or Wings node. Leave GameLink running with the
   existing server entry. After the host comes back, confirm the agent and
   game server are running, then wait for GameLink to return to **Connected**.
   Record whether the tunnel recovered by itself or needed **Retry**. Rejoin
   from the game using the current local address. Confirm gameplay works
   again with the same invitation and approval, and record the recovered path.
4. **Exercise port fallback.** Exit the game session and disconnect GameLink.
   Leave **Local port** empty. In a separate PowerShell window on the Windows
   client, bind the preferred port in the table above with the matching
   protocol. Substitute the port if the registration uses a custom preference.

   ```powershell
   # TCP example for Java. For the Bedrock TCP test, use 19132.
   $tcp = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 25565)
   $tcp.ExclusiveAddressUse = $true
   $tcp.Start()
   # Release after the test with $tcp.Stop()
   ```

   ```powershell
   # UDP example for Factorio. For the Bedrock UDP test, use 19132.
   $udp = [Net.Sockets.UdpClient]::new([Net.Sockets.AddressFamily]::InterNetwork)
   $udp.ExclusiveAddressUse = $true
   $udp.Client.Bind([Net.IPEndPoint]::new([Net.IPAddress]::Loopback, 34197))
   # Release after the test with $udp.Dispose()
   ```

   Keep the socket open and select **Join**. Confirm the busy-port notice and
   that the card shows a different local port on the requested path. Update
   the in-game address or saved server entry to that port. Join and confirm
   gameplay. For Bedrock, disconnect and repeat with the other protocol
   occupying the preferred port by itself. Release the test sockets,
   disconnect, then join again and confirm the preferred port is used.

## Record the result

Record the UTC date, desktop and agent versions or commits, tester, and result
for each run. Include the game versions, host type, observed paths, preferred
and fallback ports, revocation and restart timings, any manual recovery steps,
and the outcome of each check. If any step fails or cannot run, record the
failure or blocker and leave that combination unverified. Use redacted
diagnostics and omit invitations, keys, tokens, and Tailcat addresses from
shared evidence.
