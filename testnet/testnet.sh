#!/usr/bin/env bash

set -Eeuo pipefail

# client and server addresses
client_address="10.0.100.10"
server_address="10.0.200.10"

# pocketscion addresses
pocketscion_client_address="10.0.100.20"
pocketscion_server_address="10.0.200.20"

# mac addresses of interfaces
pocketscion_client_mac="00:76:65:74:68:30"
pocketscion_server_mac="00:76:65:74:68:31"
clientmac="00:76:65:74:68:14"
servermac="00:76:65:74:68:24"

# namespaces representing the different machines
client_ns="client_ns"
server_ns="server_ns"
pocketscion_ns="pocketscion_ns"

# interface names
client="client"
server="server"
ps_client="ps_client"
ps_server="ps_server"


# setup test network for one side
function net_up() {

	sudo ip -n $nodexns link set dev $nodex0 up

	sudo ip -n $nodexns link set dev lo up

	sudo ip -n $ehxns link set dev $ehx mtu 1500

	sudo ip -n $ehxns route add default via $nodex0_address
}

function testnet_up() {
	# create namespaces
	sudo ip netns add $pocketscion_ns
    sudo ip netns add $client_ns
    sudo ip netns add $server_ns

	# create veth pairs between client and pocketscion
	sudo ip link add $client address $clientmac type veth peer name $ps_client address $pocketscion_client_mac
	sudo ip link set dev $client netns $client_ns
	sudo ip link set dev $ps_client netns $pocketscion_ns

	# create veth pairs between server and pocketscion
	sudo ip link add $server address $servermac type veth peer name $ps_server address $pocketscion_server_mac
	sudo ip link set dev $server netns $server_ns
	sudo ip link set dev $ps_server netns $pocketscion_ns

	# configure pocketscion namespace interfaces
	sudo ip -n $client_ns address add $client_address/24 dev $client
	sudo ip -n $server_ns address add $server_address/24 dev $server
	sudo ip -n $pocketscion_ns address add $pocketscion_client_address/24 dev $ps_client
	sudo ip -n $pocketscion_ns address add $pocketscion_server_address/24 dev $ps_server

	# bring up interfaces
	sudo ip -n $client_ns link set dev $client up
	sudo ip -n $server_ns link set dev $server up
	sudo ip -n $pocketscion_ns link set dev $ps_client up
	sudo ip -n $pocketscion_ns link set dev $ps_server up
	sudo ip -n $pocketscion_ns link set dev lo up
	sudo ip -n $client_ns link set dev lo up
	sudo ip -n $server_ns link set dev lo up

}

function net_down() {
	sudo ip netns del $client_ns 2>/dev/null || true
	sudo ip netns del $server_ns 2>/dev/null || true
	sudo ip netns del $pocketscion_ns 2>/dev/null || true
}

function testnet_down() {
	net_down
}

function cleanup() {
	set +eu pipefail

	echo "perform cleanup"

	sudo ip netns del $client_ns 2>/dev/null || true
	sudo ip netns del $server_ns 2>/dev/null || true
	sudo ip netns del $pocketscion_ns 2>/dev/null || true

	sudo ip link delete $client 2>/dev/null || true
	sudo ip link delete $server 2>/dev/null || true
	sudo ip link delete $ps_client 2>/dev/null || true
	sudo ip link delete $ps_server 2>/dev/null || true
}

trap 'catch $? $LINENO' EXIT
catch() {
  if [ "$1" != "0" ]; then
		echo "Something Failed!"
    echo "Error $1 occurred on $2"
		cleanup
		exit 1
  fi
}


function usage() {
	echo "Usage:"
	echo "$0 up|down"
}

if [ $# -eq 0 ]
then
	echo "No argument provided."
	usage
	exit 1
fi

up_down=$1
if [ "$up_down" = "up" ];
then
	testnet_up
elif [ "$up_down" = "down" ];
then
	testnet_down
else
	echo "First argument must either be up or down"
	usage
	exit 1
fi

exit 0