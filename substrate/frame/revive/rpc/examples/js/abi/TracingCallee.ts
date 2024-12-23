export const TracingCalleeAbi = [
  {
    anonymous: false,
    inputs: [
      {
        indexed: true,
        internalType: "address",
        name: "caller",
        type: "address",
      },
    ],
    name: "GasConsumed",
    type: "event",
  },
  {
    inputs: [],
    name: "consumeGas",
    outputs: [],
    stateMutability: "nonpayable",
    type: "function",
  },
  {
    inputs: [],
    name: "failingFunction",
    outputs: [],
    stateMutability: "pure",
    type: "function",
  },
] as const;
