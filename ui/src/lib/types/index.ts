export interface HostOwner {
	readonly type: 'host';
	readonly service: string;
}

export interface VmOwner {
	readonly type: 'vm';
	readonly vm: string;
	readonly service: string;
}

export interface UnknownOwner {
	readonly type: 'unknown';
	readonly raw: string;
}

export type ParsedOwner = HostOwner | VmOwner | UnknownOwner;

export type ReservationState = 'pending' | 'active' | 'failed' | 'released';

export interface ReservationViewModel {
	readonly id: string;
	readonly owner: ParsedOwner;
	readonly ownerRaw: string;
	readonly assignedPort: number;
	readonly requestedPort: number | null;
	readonly targetHost: string;
	readonly targetPort: number;
	readonly state: ReservationState;
	readonly createdAt: Date;
	readonly updatedAt: Date;
	readonly expiresAt: Date | null;
	readonly leaseSeconds: number | null;
	readonly reconnectCount: number;
}

export interface TunnelHealthViewModel {
	readonly alive: boolean;
	readonly lastCheck: Date | null;
	readonly uptimeSeconds: number;
	readonly reconnectCount: number;
}

export interface SSEReservationEvent {
	reservationId: string;
	oldState: string;
	newState: string;
	timestamp: string | null;
	message: string | null;
}

export const UI_IDLE = Symbol('idle');
export const UI_RELEASING = Symbol('releasing');
export type UITransientState = typeof UI_IDLE | typeof UI_RELEASING;
