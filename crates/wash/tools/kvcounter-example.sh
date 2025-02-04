#!/bin/bash

##
# KVCounter wasmcloud example
#
# This example starts our `KVCounter` component, `httpserver` provider and `redis` provider.
#
# The component simply accepts HTTP requests and increments the value at a key matching the HTTP path.
# e.g., running `curl localhost:8080/mycounter` will add 1 to the redis key `:mycounter`
#
# Please ensure you either run `redis-server` and `nats-server` or use the included
# `docker-compose.yml` to run both of these services before you run this example.
##

if ! command -v nc &> /dev/null
then
  echo "`nc` program not found. Not able to check for redis and nats"
else
	# Check if redis is running
	nc localhost 6379 -vz &> /dev/null
	if [ $? -ne 0 ]
	then
		echo "Redis not found on localhost:6379, please ensure redis is running"
		exit
	fi

	# Check if nats is running
	nc localhost 4222 -vz &> /dev/null
	if [ $? -ne 0 ]
	then
		echo "NATS not found on localhost:4222, please ensure nats is running"
		exit
	fi
fi

if ! command -v wash &> /dev/null
then
	echo "`wash` not found in your path, please install wash or move it to your path"
	exit
fi

echo "Discovering hosts ..."
HOSTS=$(wash ctl get hosts -o json)
if [ $(echo $HOSTS | jq '.hosts | length') -gt 0 ] 
then
	# The following commands can be run as-is if you have a running wasmcloud host.
	# If you don't, you can omit the `wash` part of the command, and run the `ctl` commands in the REPL.
	HOST=$(echo $HOSTS | jq ".hosts[0].id" | tr -d "\"")
	wash ctl start component wasmcloud.azurecr.io/kvcounter:0.2.0 -h $HOST
	wash ctl start provider wasmcloud.azurecr.io/redis:0.10.0 -h $HOST
	wash ctl link MCFMFDWFHGKELOXPCNCDXKK5OFLHBVEWRAOXR5JSQUD2TOFRE3DFPM7E VAZVC4RX54J2NVCMCW7BPCAHGGG5XZXDBXFUMDUXGESTMQEJLC3YVZWB wasmcloud:keyvalue URL=redis://localhost:6379
	wash ctl start provider wasmcloud.azurecr.io/httpserver:0.10.0 -h $HOST
	wash ctl link MCFMFDWFHGKELOXPCNCDXKK5OFLHBVEWRAOXR5JSQUD2TOFRE3DFPM7E VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M wasmcloud:httpserver PORT=8080

	echo ""
	echo "Components and providers linked and starting, try running one of the following commands to test your KVCounter!"
	echo "curl localhost:8080/mycounter
wash ctl call MCFMFDWFHGKELOXPCNCDXKK5OFLHBVEWRAOXR5JSQUD2TOFRE3DFPM7E HandleRequest '{\"method\": \"GET\", \"path\": \"/mycounter\", \"body\": \"\", \"queryString\":\"\", \"header\":{}}'"
else
	echo "No hosts found, please run the wasmcloud binary, or proceed with the following commands in the REPL:"
	echo ""
	echo "ctl start component wasmcloud.azurecr.io/kvcounter:0.2.0
ctl start provider wasmcloud.azurecr.io/redis:0.10.0
ctl link MCFMFDWFHGKELOXPCNCDXKK5OFLHBVEWRAOXR5JSQUD2TOFRE3DFPM7E VAZVC4RX54J2NVCMCW7BPCAHGGG5XZXDBXFUMDUXGESTMQEJLC3YVZWB wasmcloud:keyvalue URL=redis://localhost:6379
ctl start provider wasmcloud.azurecr.io/httpserver:0.10.0
ctl link MCFMFDWFHGKELOXPCNCDXKK5OFLHBVEWRAOXR5JSQUD2TOFRE3DFPM7E VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M wasmcloud:httpserver PORT=8080
ctl call MCFMFDWFHGKELOXPCNCDXKK5OFLHBVEWRAOXR5JSQUD2TOFRE3DFPM7E HandleRequest {\"method\": \"GET\", \"path\": \"/mycounter\", \"body\": \"\", \"queryString\":\"\", \"header\":{}}"
	exit
fi

