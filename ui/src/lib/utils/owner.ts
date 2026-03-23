import type { ParsedOwner } from '$lib/types/index.js';

const OWNER_REGEX = /^[a-zA-Z0-9:\-_.]{1,128}$/;

export function parseOwner(raw: string): ParsedOwner {
	const parts = raw.split(':');
	if (parts[0] === 'host' && parts.length === 2 && parts[1]) {
		return { type: 'host', service: parts[1] };
	}
	if (parts[0] === 'vm' && parts.length >= 3 && parts[1] && parts[2]) {
		return { type: 'vm', vm: parts[1], service: parts.slice(2).join(':') };
	}
	return { type: 'unknown', raw };
}

export function ownerDisplayLabel(owner: ParsedOwner): string {
	switch (owner.type) {
		case 'host':
			return owner.service;
		case 'vm':
			return `${owner.vm}/${owner.service}`;
		case 'unknown':
			return owner.raw;
	}
}

export function ownerTypeBadge(owner: ParsedOwner): string {
	switch (owner.type) {
		case 'host':
			return 'HOST';
		case 'vm':
			return 'VM';
		case 'unknown':
			return '?';
	}
}

export function assembleOwner(
	ownerType: 'host' | 'vm',
	vmName: string,
	serviceName: string
): string {
	if (ownerType === 'host') {
		return `host:${serviceName}`;
	}
	return `vm:${vmName}:${serviceName}`;
}

export function validateOwner(owner: string): string | null {
	if (!owner) return 'Owner is required';
	if (owner.length > 128) return 'Owner must be 128 characters or less';
	if (!OWNER_REGEX.test(owner)) return 'Owner contains invalid characters';
	return null;
}
