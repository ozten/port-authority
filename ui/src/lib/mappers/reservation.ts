import type { ReservationViewModel, ReservationState, TunnelHealthViewModel } from '$lib/types/index.js';
import { parseOwner } from '$lib/utils/owner.js';

const STATE_MAP: Record<number, ReservationState> = {
	1: 'pending',
	2: 'active',
	3: 'failed',
	4: 'released'
};

function timestampToDate(ts: { seconds: { low: number; high: number }; nanos: number } | undefined): Date {
	if (!ts) return new Date(0);
	// Long has low/high, convert to number. For timestamps in reasonable range, low is sufficient.
	const seconds = ts.seconds.low + ts.seconds.high * 0x100000000;
	return new Date(seconds * 1000 + ts.nanos / 1_000_000);
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function toReservationViewModel(proto: any): ReservationViewModel {
	return {
		id: proto.id,
		owner: parseOwner(proto.owner),
		ownerRaw: proto.owner,
		assignedPort: proto.assignedPort,
		requestedPort: proto.requestedPort || null,
		targetHost: proto.targetHost,
		targetPort: proto.targetPort,
		state: STATE_MAP[proto.state] ?? 'pending',
		createdAt: timestampToDate(proto.createdAt),
		updatedAt: timestampToDate(proto.updatedAt),
		expiresAt: proto.expiresAt ? timestampToDate(proto.expiresAt) : null,
		leaseSeconds: proto.leaseSeconds ?? null,
		reconnectCount: proto.reconnectCount ?? 0
	};
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function toTunnelHealthViewModel(proto: any): TunnelHealthViewModel {
	return {
		alive: proto.alive ?? false,
		lastCheck: proto.lastCheck ? timestampToDate(proto.lastCheck) : null,
		uptimeSeconds: proto.uptimeSeconds ?? 0,
		reconnectCount: proto.reconnectCount ?? 0
	};
}

export function serializeReservation(vm: ReservationViewModel): Record<string, unknown> {
	return {
		id: vm.id,
		ownerRaw: vm.ownerRaw,
		ownerType: vm.owner.type,
		ownerLabel:
			vm.owner.type === 'host'
				? vm.owner.service
				: vm.owner.type === 'vm'
					? `${vm.owner.vm}/${vm.owner.service}`
					: vm.ownerRaw,
		vmName: vm.owner.type === 'vm' ? vm.owner.vm : null,
		assignedPort: vm.assignedPort,
		requestedPort: vm.requestedPort,
		targetHost: vm.targetHost,
		targetPort: vm.targetPort,
		state: vm.state,
		createdAt: vm.createdAt.toISOString(),
		updatedAt: vm.updatedAt.toISOString(),
		expiresAt: vm.expiresAt?.toISOString() ?? null,
		leaseSeconds: vm.leaseSeconds,
		reconnectCount: vm.reconnectCount
	};
}
