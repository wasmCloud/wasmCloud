#!/bin/bash

host_data='{"lattice_rpc_url": "0.0.0.0:4222", "lattice_rpc_prefix": "default", "provider_key": "keyvalue-inmemory", "link_name": "default","log_level": "debug"}' 
echo $host_data | base64 | go run ./
