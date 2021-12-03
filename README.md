# Substrate ecosystem JSON-RPC interface specification

This repository contains the specification of the JSON-RPC interface exposed by blockchain nodes in the Substrate ecosystem.

This specification aims at establishing a *lingua franca* between UIs, tools, etc. that would like to access a Substrate-based blockchain, and nodes client implementations that actually connect to the blockchain. Please note that it is, however, in no way mandatory for nodes to implement this specification in order to access a Substrate-based blockchain. As such, this specification is out of scope of, say, the Polkadot host specification.

The content of this repository is automatically uploaded to: https://paritytech.github.io/json-rpc-interface-spec/

## Contributing

This repository contains the specification.

It aims at being a reference point. If the text in this specification doesn't match what an implementation is doing, it is this specification that is correct.

You are encouraged to open issues if you see issues or would like to suggest improvements to this specification, or if some elements are missing clarity.
This specification can be modified (in a backwards-compatible way) by opening pull requests.

## Note

*Creating this repository is the first point.*

At the moment, this specification isn't implemented by Substrate or any other client implementation.
It is planned to be implemented in Substrate once this specification has been fully fleshed out.

[The smoldot repository](https://github.com/paritytech/smoldot) aims to implement this specification while it is still in progress, in order to experiment with it in practice.
