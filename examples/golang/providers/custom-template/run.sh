#!/bin/bash

host_data='{"lattice_rpc_url": "0.0.0.0:4222", "lattice_rpc_prefix": "default", "provider_key": "custom-template", "link_name": "default", "structured_logging": true}'
echo -n "$host_data" | base64 -w 0 | go run ./
