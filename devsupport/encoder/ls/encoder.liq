#!/usr/bin/env liquidsoap
run_dir = environment.get(default="/encoder/run", "RUN_DIR")
id = int_of_string(environment.get(default="0", "ENCODER_ID"))
uid = "enc-#{id}"
args = environment.get(
  default="--bitrate 72 --rate 48000 --channels 2",
  "AUDIOENC_ARGS"
)
src = environment.get(default="", "SRC")

mux_port = 9000 + id

# docker root
settings.init.allow_root := true

# audio
settings.frame.audio.channels := 2
settings.frame.audio.samplerate := 48000

# logging
settings.log.level := 2
settings.log.stdout := true
settings.log.file := false

let command = "/usr/local/bin/odr-audioenc -i - -f raw #{args} --pad-socket #{uid} -e tcp://mux:#{mux_port}"

print("odr-audioenc cmd: #{command}")

output.external(
    id='output_audioenc',
    %ffmpeg(
      format="s16le",
      %audio(
        codec="pcm_s16le",
        ac=2,
        ar=48000
      )
    ),
    fallible=true,
    flush=true,
    self_sync=true,
    command,
    mksafe(
      input.http(
        max_buffer=3.,
        poll_delay=10.,
        self_sync=false,
        src
      )
    )
)
