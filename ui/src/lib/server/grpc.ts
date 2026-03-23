import { createChannel, createClient, type Channel, type Client } from 'nice-grpc';
import { PortBrokerDefinition } from './generated/portd.js';

type PortBrokerClient = Client<typeof PortBrokerDefinition>;

const GRPC_KEY = Symbol.for('portd.grpc');

interface GrpcSingleton {
	channel: Channel;
	client: PortBrokerClient;
}

function getGrpc(): GrpcSingleton {
	const existing = (globalThis as Record<symbol, GrpcSingleton | undefined>)[GRPC_KEY];
	if (existing) return existing;

	const runtimeDir = process.env.XDG_RUNTIME_DIR || '/run/user/1000';
	const socketPath = process.env.PORTD_SOCKET || `${runtimeDir}/portd.sock`;
	const address = `unix:${socketPath}`;

	const channel = createChannel(address);
	const singleton: GrpcSingleton = {
		channel,
		client: createClient(PortBrokerDefinition, channel)
	};

	(globalThis as Record<symbol, GrpcSingleton>)[GRPC_KEY] = singleton;

	return singleton;
}

export function getClient(): PortBrokerClient {
	return getGrpc().client;
}

export function closeChannel(): void {
	const existing = (globalThis as Record<symbol, GrpcSingleton | undefined>)[GRPC_KEY];
	if (existing) {
		existing.channel.close();
		delete (globalThis as Record<symbol, GrpcSingleton | undefined>)[GRPC_KEY];
	}
}

// Graceful shutdown
if (typeof process !== 'undefined') {
	process.on('sveltekit:shutdown' as string, () => {
		closeChannel();
	});
}
