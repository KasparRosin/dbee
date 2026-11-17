# dbee's development environment. Built and started by `dev/run`; see dev/README.md.

FROM ubuntu:26.04
LABEL dev.dbee.image=dev
ENV LANG=C.UTF-8


# 1. System packages

ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    # Tools.
        build-essential \
        ca-certificates \
        curl \
        git \
        less \
        openssh-client \
        pkg-config \
        procps \
    # The libraries dbee's window and fonts link against, as in CI (.github/workflows/ci.yml).
        libfontconfig1-dev \
        libfreetype-dev \
        libwayland-dev \
        libx11-dev \
        libxcursor-dev \
        libxi-dev \
        libxkbcommon-dev \
        libxrandr-dev \
        mesa-vulkan-drivers \
    # A virtual screen for the tests that open a real window (dev/README.md). winit loads
    # libxkbcommon-x11 to open a window on X11, and stops without it.
        libxkbcommon-x11-0 \
        xauth \
        xvfb \
    && rm -rf /var/lib/apt/lists/*


# 2. The Docker client, to start the test servers on the host's Docker (no Docker server inside)

COPY --from=docker:29-cli /usr/local/bin/docker /usr/local/bin/
COPY --from=docker:29-cli /usr/local/libexec/docker/cli-plugins/ /usr/local/libexec/docker/cli-plugins/


# 3. Rust, at the version in rust-toolchain.toml

ARG RUST_VERSION
ENV RUSTUP_HOME=/usr/local/rustup
RUN curl --proto =https -sSf https://sh.rustup.rs | CARGO_HOME=/usr/local/cargo sh -s -- -y \
        --no-modify-path \
        --profile minimal \
        --default-toolchain "$RUST_VERSION" \
        --component clippy \
        --component rustfmt


# 4. The host's user, with the same uid, gid and home path, so files and paths match the host's

ARG USER_NAME
ARG USER_UID
ARG USER_GID
ARG HOME_DIR

# Ubuntu's own user `ubuntu` holds uid 1000, so it goes.
RUN userdel --remove ubuntu
RUN groupadd --gid "$USER_GID" "$USER_NAME"
RUN useradd --create-home --home-dir "$HOME_DIR" --uid "$USER_UID" --gid "$USER_GID" --shell /bin/bash "$USER_NAME"


# 5. Start as that user

COPY entry /usr/local/bin/dbee-dev-entry

USER $USER_NAME
# Folders that hold files mounted from the host, made now so they belong to the user.
RUN mkdir -p "$HOME_DIR/.config" "$HOME_DIR/.ssh"

ENV CARGO_HOME=$HOME_DIR/.cargo
ENV CLAUDE_CONFIG_DIR=$HOME_DIR/.claude
ENV PATH=$HOME_DIR/.local/bin:/usr/local/cargo/bin:$PATH

WORKDIR $HOME_DIR
ENTRYPOINT ["/usr/local/bin/dbee-dev-entry"]
CMD ["bash"]
