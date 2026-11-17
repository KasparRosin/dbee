# Development container

A container with everything dbee's build and tests need, for a machine that should hold only Docker: the Rust release
in `rust-toolchain.toml` with clippy and rustfmt, the libraries CI installs, git, the Docker client with Compose, and
Claude Code. The test servers (`cargo xtask testdb`) run on the host's Docker beside it, as they do without the
container.

## Start it

```sh
dev/run                  # a shell in the container
dev/run cargo xtask ci   # one command
```

`dev/run` builds the image `dbee-dev` first (the first time takes a few minutes, later runs reuse it) and starts a
container that is removed when its command ends. What has to last lives on the host, so a new container picks it up.
Run the `dev/run` of the clone, also from a worktree of it: the container then holds the clone with all its worktrees.

## It is not a sandbox

The container keeps the tools off the host. It does not keep what runs inside from the host:

- It has the host's Docker socket, and whoever can use that can start a container with the host's disk in it. Treat
  anything that runs inside, Claude Code included, as able to act as root on the host.
- It shares the host's network, so it reaches every service listening on the host's `127.0.0.1`.
- Without an SSH agent that holds your keys, it can read your SSH private keys.

## What the host provides

- Docker Engine running as root, with Compose and Buildx, and your user in the `docker` group: the distribution's
  `docker.io` package or Docker's own packages, not the snap and not rootless Docker. The container uses the host's
  Docker through `/var/run/docker.sock`, with the socket's group taken at each start.
- The repository, mounted at its own path, so paths `cargo xtask` hands to the host's Docker mean the same there.
  `target/` stays in it, on the host's disk.
- From your home, at each start:
  - your git settings, read-only, with the files they include;
  - SSH: the agent's socket when `SSH_AUTH_SOCK` names an agent that holds keys, with `~/.ssh/known_hosts`
    read-only; otherwise `~/.ssh` read-only. A host GitHub has not been seen from yet needs its key in
    `~/.ssh/known_hosts` first (`ssh -T git@github.com` once on the host);
  - `~/.local/share/dbee-dev` (or `DBEE_DEV_STATE`): Claude Code's install, sign-in and settings, and the containers'
    own Cargo home with its downloads. The first start installs Claude Code there. Sign in with `claude` inside the
    container once; removing the container keeps it.

The container runs as your user, with your uid, gid and home path, on the host's network: the test servers' ports on
`127.0.0.1` are the host's.

## Notes

- A new release in `rust-toolchain.toml` builds a new image at the next `dev/run`.
- `CARGO_TARGET_DIR`, when set, must be inside the repository: anything else would be lost when the container ends.
  On a machine that also builds outside the container, give the container its own build folder, so the two don't
  rebuild each other's files: `CARGO_TARGET_DIR=target-dev dev/run` (`target-*/` is ignored by git).
- `nice` on `dev/run` doesn't reach the container, and the host's Cargo settings aren't read inside. To build with fewer
  jobs, set `CARGO_BUILD_JOBS` when starting it.
- The tests that open a real window (the M1 and M2 workflow tests and the control channel test in `apps/dbee/tests`,
  and the launch time check) need a display. `cargo xtask ci` doesn't run them. The container has Xvfb, a virtual X11
  screen in memory, to run them on:

  ```sh
  xvfb-run -a -s "-screen 0 1920x1080x24" cargo test -p dbee --test m1_day --test m2_day --test control_access -- --ignored --nocapture
  xvfb-run -a -s "-screen 0 1920x1080x24" cargo test -p xtask connection_screen -- --ignored --nocapture   # launch time
  ```

  The days save their screenshots in `target/m1-day/` and `target/m2-day/`. Xvfb runs no window manager, so winit
  logs `BadWindow` X11 errors while the app runs; they don't affect it. This is X11, not Wayland: behaviour that only
  shows on a Wayland compositor (`docs/testing.md` §4) still needs a desktop.
