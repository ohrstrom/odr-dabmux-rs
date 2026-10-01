FROM savonet/liquidsoap:v2.3.3-minimal

USER root

WORKDIR /encoder/

ENV DEBIAN_FRONTEND=noninteractive \
    PYTHONUNBUFFERED=1

RUN > /etc/apt/sources.list

RUN set -eux && \
    apt-get update && \
    apt-get install -y \
        git \
        curl \
        inotify-tools \
        supervisor \
        build-essential \
        automake \
        libtool \
        libzmq3-dev \
        libzmq5 \
        libcurl4-openssl-dev \
        libmagickwand-dev


# ODR-AudioEnc
RUN git clone https://github.com/Opendigitalradio/ODR-AudioEnc.git && \
  cd ODR-AudioEnc && \
  ./bootstrap && \
  ./configure && \
  make && \
  make install

# ODR-PadEnc
RUN git clone https://github.com/Opendigitalradio/ODR-PadEnc.git && \
  cd ODR-PadEnc && \
  ./bootstrap && \
  ./configure && \
  make && \
  make install

RUN mkdir -p /encoder/supervisor.d/

COPY \
  encoder/supervisor.conf \
  ./

ADD \
  encoder/ls \
  ls

ENTRYPOINT ["/usr/bin/supervisord"]

CMD ["-c", "/encoder/supervisor.conf", "-n"]