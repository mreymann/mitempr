# mitempr

Read data from Bluetooth environmental sensors in BTHome v2, PVVX and LYWSDCGQ formats.  
Strongly inspired by [Mitemperature2](https://github.com/JsBergbau/MiTemperature2). Thank you, JsBergbau!

## Why

 - learn a bit of Rust
 - prove how much AI can help an idiot (like me)
 - try to use less resources on my poor Pi Zero W

## Status

 - nicely cross compiles to armv6 (Pi Zero W), armv7 (Pi Zero W 2)
 - continuous scanning now works!

## TODOs

 - get this darn thing to be more responsive (#bluez)
 - also decode **encrypted** data
 - URL callback to Prometheus Push Gateway
 - call external scripts
 - define sensors in a config file & filter defined sensors
 - add flags and options to binary
 - and many more things to fiddle with ;-)

## Cross compiling

### Pi Zero W 1
- `cross build --release --target=arm-unknown-linux-musleabihf`

### Pi Zero W 2
- `cross build --release --target aarch64-unknown-linux-musl`