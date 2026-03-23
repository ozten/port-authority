import { createChannel, createClient, type Channel, type Client } from 'nice-grpc';
import { homedir } from 'os';
import { join } from 'path';
import { PortBrokerDefinition } from './generated/portd.js';

type PortBrokerClient = Client<typeof PortBrokerDefinition>;

const GRPC_KEY = Symbol.for('portd.grpc');

interface GrpcSingleton {
	channel: Channel;
	client: PortBrokerClient;
}

function resolveSocketPath(): string {
	// Explicit override via env var
	if (process.env.PORTD_SOCKET) return process.env.PORTD_SOCKET;

	// Match the daemon's default_socket_path logic:
	// 1. $XDG_RUNTIME_DIR/portd.sock (Linux)
	// 2. ~/.local/share/portd/portd.sock (macOS fallback)
	if (process.env.XDG_RUNTIME_DIR) {
		return join(process.env.XDG_RUNTIME_DIR, 'portd.sock');
	}
	return join(homedir(), '.local', 'share', 'portd', 'portd.sock');
}

function getGrpc(): GrpcSingleton {
	const existing = (globalThis as Record<symbol, GrpcSingleton | undefined>)[GRPC_KEY];
	if (existing) return existing;

	const socketPath = resolveSocketPath();
	const address = `unix://${socketPath}`;

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
