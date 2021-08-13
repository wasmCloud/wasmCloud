This repository contains capability providers for wasmcloud.
The files are currently under construction in preparation for the
wasmcloud 0.50 release with otp.

Capability providers compatible with wasmcloud host 0.18
can be found in the [pre-otp](./pre-otp) folder.

Capability providers compatible with OTP wasmcloud host are

- Http Server [httpserver](./httpserver-rs)
  - for the 'wasmcloud:httpserver' capability contract
  - built in rust with warp/hyper engine
  
- Key Value [kvredis](./kvredis)
  - for the 'wasmcloud:keyvalue' capability contract
  - bulit in rust with Redis

