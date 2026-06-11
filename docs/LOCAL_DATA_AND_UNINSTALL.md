# Local Data And Uninstall

Fleet source alpha writes only local runtime data. It has no intended telemetry
by default.

## Runtime Data Locations

- `~/.fleet/run`: embedded Hub lock, socket, token, and host log.
- `~/.fleet/mux`: Fleet-spawned editor workspaces, server logs, VS Code
  `--server-data-dir` userdata, reporter sockets, and Claude shim files.

Environment overrides:

- `FLEET_RUNTIME_DIR` changes the Hub runtime/log directory.
- `FLEET_MUX_DIR` changes spawned editor workspace, userdata, and log storage.

## Process Ownership

Fleet is a stateless client for live sessions. Externally registered sessions push state to Fleet and are not owned by the host. Quitting Fleet must not kill external servers.

The host may create local `code serve-web` sessions as a convenience. Closing a Fleet-spawned server from the Fleet UI is the explicit action that removes that server process. Quitting Fleet does not promise to delete spawned editor userdata or logs.

## Manual Cleanup

Close any Fleet-spawned servers from the Fleet UI before deleting runtime data.
Then remove the source-alpha runtime directories:

```sh
rm -rf ~/.fleet/run ~/.fleet/mux
```

If `FLEET_RUNTIME_DIR` or `FLEET_MUX_DIR` was set, delete those configured
directories instead.

Deleting `~/.fleet/mux` removes Fleet-created editor workspaces, logs, reporter
sockets, generated VS Code server userdata, and Claude shim files. It does not
remove the user's VS Code installation or repositories outside the Fleet runtime
directories.
