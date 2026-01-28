# Stage 1: Build environment
FROM ubuntu:26.04 AS builder

# Avoid prompts during apt installations
ENV DEBIAN_FRONTEND=noninteractive

ARG REBUILD_HNSWLIB

# Upgrading OpenSSL to fix CVE-2025-9230
RUN apt-get update && \
    apt-get install -y --no-install-recommends --only-upgrade openssl libssl3t64 && \
    apt-get dist-upgrade -y && \
    rm -rf /var/lib/apt/lists/* && \
    mkdir /install

WORKDIR /install

# Install system dependencies required for building Python and the app
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    gcc g++ \
    make \
    cmake \
    autoconf \
    git \
    wget \
    curl \
    ca-certificates \
    libssl-dev zlib1g-dev libbz2-dev \
    libreadline-dev libsqlite3-dev \
    libffi-dev liblzma-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*


# Install pyenv
ENV PYENV_ROOT="/opt/python3.12"
ENV PATH="$PYENV_ROOT/bin:$PATH"
RUN git clone https://github.com/pyenv/pyenv.git $PYENV_ROOT

# Install Python 3.12.0 using pyenv
RUN $PYENV_ROOT/bin/pyenv install 3.12.0 && \
    $PYENV_ROOT/bin/pyenv global 3.12.0

# Remove vulnerable setuptools to address CVE-2025-47273
RUN rm -f /opt/python3.12/versions/3.12.0/lib/python3.12/test/setuptools-67.6.1-py3-none-any.whl | true

# Create and activate Python virtual environment using pyenv's python
RUN /opt/python3.12/versions/3.12.0/bin/python -m venv /install/.venv && \
    /opt/python3.12/versions/3.12.0/bin/python3 -m pip install --no-cache-dir --upgrade pip==25.2

ENV PATH="/install/.venv/bin:$PATH"

# Remove vulnerable pip to address CVE-2023-5752
RUN rm -f /opt/python3.12/versions/3.12.0/lib/python3.12/ensurepip/_bundled/pip-23.2.1-py3-none-any.whl | true && \
    rm -rf /opt/python3.12/versions/3.12.0/lib/python3.12/site-packages/pip-23.2.1.dist-info | true && \
    rm -rf /opt/python3.12/versions/3.12.0/lib/python3.12/site-packages/pip-23.2.1.dist-info | true && \
    rm -f /opt/python3.12/versions/3.12.0/lib/python3.12/test/wheel-0.40.0-py3-none-any.whl | true && \
    rm -rf /install/.venv/lib/python3.12/site-packages/pip-23.2.1.dist-info | true && \
    rm -rf /install/.venv/lib/python3.12/site-packages/orjson-3.10.5.dist-info | true && \
    rm -rf /install/.venv/lib/python3.12/site-packages/protobuf-5.28.0.dist-info | true && \
    rm -rf /opt/python3.12/versions/3.12.0/lib/python3.12/site-packages/pip-25.2.dist-info | true && \
    rm -rf /install/.venv/lib/python3.12/site-packages/black-23.3.0.dist-info | true && \
    rm -rf /install/.venv/lib/python3.12/site-packages/protobuf-6.33.4.dist-info | true && \
    rm -rf /install/.venv/lib/python3.12/site-packages/orjson-3.10.5.dist-info | true && \
    rm -rf /install/.venv/lib/python3.12/site-packages/protobuf-5.28.0.dist-info


COPY ./requirements.txt requirements.txt

RUN --mount=type=cache,target=/root/.cache/pip /install/.venv/bin/pip install --upgrade -r requirements.txt && \
    rm -rf /install/.venv/lib/python3.12/site-packages/orjson-3.10.5.dist-info \
           /install/.venv/lib/python3.12/site-packages/protobuf-6.33.4.dist-info
RUN --mount=type=cache,target=/root/.cache/pip if [ "$REBUILD_HNSWLIB" = "true" ]; then /install/.venv/bin/pip install --no-binary :all: --force-reinstall chroma-hnswlib; fi
RUN rm -f requirements.txt 
RUN rm -f requirements_dev.txt | true

FROM ubuntu:26.04 AS final

# Avoid prompts during apt installations
ENV DEBIAN_FRONTEND=noninteractive

# Upgrading OpenSSL to fix CVE-2025-9230
RUN apt-get update && \
    apt-get install -y --no-install-recommends --only-upgrade openssl libssl3t64 && \
    apt-get dist-upgrade -y && \
    rm -rf /var/lib/apt/lists/*

RUN mkdir /chroma
WORKDIR /chroma

# Install only minimal required system dependencies for runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libpq5 \
    dos2unix \
    libssl-dev \
    zlib1g-dev \
    libbz2-dev \
    libreadline-dev \
    libsqlite3-dev \
    libncurses5-dev \
    libncursesw5-dev \
    xz-utils \
    libffi-dev \
    liblzma-dev \
    && rm -rf /var/lib/apt/lists/*


COPY --from=builder /opt/python3.12 /opt/python3.12

COPY ./bin/docker_entrypoint.sh /docker_entrypoint.sh

RUN apt-get update --fix-missing && apt-get install -y curl dos2unix && \
    dos2unix /docker_entrypoint.sh && \
    chmod +x /docker_entrypoint.sh && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /install /install
COPY ./ /chroma

ENV CHROMA_HOST_ADDR="0.0.0.0"
ENV CHROMA_HOST_PORT=8000
ENV CHROMA_WORKERS=1
ENV CHROMA_LOG_CONFIG="chromadb/log_config.yml"
ENV CHROMA_TIMEOUT_KEEP_ALIVE=30

EXPOSE 8000

ENV PATH="/install/.venv/bin:/opt/python3.12/versions/3.12.0/bin:${PATH}"

ENTRYPOINT ["/docker_entrypoint.sh"]

CMD ["uvicorn", "chromadb.app:app", "--workers", "${CHROMA_WORKERS}", "--host", "${CHROMA_HOST_ADDR}", "--port", "${CHROMA_HOST_PORT}", "--proxy-headers", "--log-config", "${CHROMA_LOG_CONFIG}", "--timeout-keep-alive", "${CHROMA_TIMEOUT_KEEP_ALIVE}"]
