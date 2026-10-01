# Development commands

`test.wav` is ignored by git; place a 48 kHz stereo WAV file here.

## Encoder

`odr-audioenc` reading a file runs faster than real time. TCP inputs throttle it with backpressure (the default), so no pacing is needed.

```shell
# config.example.yaml: input on TCP port 9000
odr-audioenc -i devsupport/test.wav --bitrate 72 --rate 48000 --channels 2 -e tcp://127.0.0.1:9000

# config.production.example.yaml: sub-rnd001 listens on TCP port 9001
odr-audioenc -i devsupport/test.wav --bitrate 72 --rate 48000 --channels 2 -e tcp://127.0.0.1:9001
```

The encoder bitrate must match the subchannel's `bitrate`.

## Mux

```shell
cargo run --bin dabmux -- --config dabmux/config.example.yaml
curl -s localhost:7777/api/stats
```

`input_drops` should stay at zero with a file encoder; the mux logs a warning when an input keeps delivering faster than real time.

## Decode the output

```shell
# config.example.yaml serves EDI on TCP port 9001 (production: 8850)
nc 127.0.0.1 9001 > /tmp/out.edi   # stop with Ctrl-C after a few seconds
dablin -f edi -1 -u /tmp/out.edi > /tmp/out.aac
```

DABlin should report one superframe sync and no `(AU #n)` errors.


## Devsupport - Containered odr-audioenc

```shell
docker compose up -d
```

### Reload script (supervisor update after config change)

```shell
uv run encoder-ctl.py update
```