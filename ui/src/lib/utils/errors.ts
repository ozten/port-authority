import { ClientError, Status } from 'nice-grpc';

export function grpcErrorToMessage(error: unknown): string {
	if (!(error instanceof ClientError)) {
		if (error instanceof Error) return error.message;
		return 'An unknown error occurred';
	}

	const detail = error.details || '';

	switch (error.code) {
		case Status.ALREADY_EXISTS:
			return detail || 'Port is already reserved';
		case Status.RESOURCE_EXHAUSTED:
			return detail || 'Resource limit reached';
		case Status.NOT_FOUND:
			return detail || 'Reservation not found';
		case Status.FAILED_PRECONDITION:
			return detail || 'Configuration issue';
		case Status.UNAVAILABLE:
			return 'Cannot reach portd daemon. Is it running?';
		case Status.INTERNAL:
			return 'Internal error — check portd logs';
		case Status.INVALID_ARGUMENT:
			return detail || 'Invalid input';
		default:
			return detail || `gRPC error (code ${error.code})`;
	}
}

export function isConnectionError(error: unknown): boolean {
	if (error instanceof ClientError) {
		return error.code === Status.UNAVAILABLE;
	}
	if (error instanceof Error) {
		return (
			error.message.includes('ECONNREFUSED') ||
			error.message.includes('ENOENT') ||
			error.message.includes('No connection established')
		);
	}
	return false;
}
