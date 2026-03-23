<script lang="ts">
	import { goto } from '$app/navigation';
	import { onDestroy } from 'svelte';

	let { data } = $props();
	let health = $state(data.tunnelHealth);
	let releasing = $state(false);

	const isVm = $derived(data.reservation.ownerType === 'vm');

	// Poll health every 10 seconds for VM reservations
	let pollInterval: ReturnType<typeof setInterval> | null = null;
	let abortController: AbortController | null = null;

	$effect(() => {
		if (!isVm || data.reservation.state !== 'active') return;
		pollInterval = setInterval(async () => {
			if (document.hidden) return;
			abortController?.abort();
			abortController = new AbortController();
			try {
				const res = await fetch(`/api/inspect/${data.reservation.id}`, {
					signal: abortController.signal
				});
				if (res.ok) {
					const result = await res.json();
					if (result.tunnelHealth) health = result.tunnelHealth;
				}
			} catch (e) {
				if (e instanceof DOMException && e.name === 'AbortError') return;
			}
		}, 10_000);

		return () => {
			if (pollInterval) clearInterval(pollInterval);
			abortController?.abort();
		};
	});

	async function handleRelease() {
		if (isVm && !confirm('This will tear down the SSH tunnel. Active connections will drop. Continue?')) {
			return;
		}
		releasing = true;
		try {
			const res = await fetch('/api/reservations', {
				method: 'DELETE',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({ reservation_id: data.reservation.id })
			});
			if (res.ok) {
				goto('/');
			}
		} finally {
			releasing = false;
		}
	}

	const stateColors: Record<string, string> = {
		pending: 'text-[var(--color-warning)]',
		active: 'text-[var(--color-success)]',
		failed: 'text-[var(--color-destructive)]',
		released: 'text-[var(--color-muted-foreground)]'
	};

	function formatUptime(seconds: number): string {
		if (seconds < 60) return `${seconds}s`;
		if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
		const h = Math.floor(seconds / 3600);
		const m = Math.floor((seconds % 3600) / 60);
		return `${h}h ${m}m`;
	}
</script>

<div class="max-w-2xl">
	<div class="mb-6 flex items-center justify-between">
		<div>
			<div class="flex items-center gap-2 mb-1">
				<a href="/" class="text-[var(--color-muted-foreground)] hover:text-[var(--color-foreground)] text-sm">&larr; Back</a>
			</div>
			<h1 class="text-xl font-bold">
				Port {data.reservation.assignedPort}
			</h1>
		</div>
		{#if data.reservation.state !== 'released'}
			<button
				onclick={handleRelease}
				disabled={releasing}
				class="rounded-md border border-red-500/20 bg-red-500/10 px-4 py-2 text-sm font-medium text-[var(--color-destructive)] hover:bg-red-500/20 transition-colors disabled:opacity-50"
			>
				{releasing ? 'Releasing...' : 'Release'}
			</button>
		{/if}
	</div>

	<!-- Info Grid -->
	<div class="rounded-lg border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
		<div class="grid grid-cols-2 gap-4 p-4">
			<div>
				<div class="text-xs text-[var(--color-muted-foreground)] mb-1">ID</div>
				<div class="font-mono text-xs select-all">{data.reservation.id}</div>
			</div>
			<div>
				<div class="text-xs text-[var(--color-muted-foreground)] mb-1">State</div>
				<div class="font-medium {stateColors[data.reservation.state] ?? ''}">
					{data.reservation.state}
				</div>
			</div>
		</div>

		<div class="grid grid-cols-2 gap-4 p-4">
			<div>
				<div class="text-xs text-[var(--color-muted-foreground)] mb-1">Owner</div>
				<div>
					<span class="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-bold uppercase tracking-wider border mr-1
						{data.reservation.ownerType === 'vm'
							? 'text-[var(--color-primary)] bg-indigo-500/10 border-indigo-500/20'
							: 'text-[var(--color-muted-foreground)] bg-zinc-500/10 border-zinc-500/20'}">
						{data.reservation.ownerType === 'vm' ? 'VM' : 'HOST'}
					</span>
					{data.reservation.ownerLabel}
				</div>
			</div>
			<div>
				<div class="text-xs text-[var(--color-muted-foreground)] mb-1">Raw Owner</div>
				<div class="font-mono text-sm">{data.reservation.ownerRaw}</div>
			</div>
		</div>

		<div class="grid grid-cols-3 gap-4 p-4">
			<div>
				<div class="text-xs text-[var(--color-muted-foreground)] mb-1">Assigned Port</div>
				<div class="font-mono font-bold text-lg">{data.reservation.assignedPort}</div>
			</div>
			{#if data.reservation.requestedPort}
				<div>
					<div class="text-xs text-[var(--color-muted-foreground)] mb-1">Requested Port</div>
					<div class="font-mono">{data.reservation.requestedPort}</div>
				</div>
			{/if}
			<div>
				<div class="text-xs text-[var(--color-muted-foreground)] mb-1">Target</div>
				<div class="font-mono">{data.reservation.targetHost}:{data.reservation.targetPort}</div>
			</div>
		</div>

		<div class="grid grid-cols-2 gap-4 p-4">
			<div>
				<div class="text-xs text-[var(--color-muted-foreground)] mb-1">Created</div>
				<div class="text-sm">{new Date(data.reservation.createdAt).toLocaleString()}</div>
			</div>
			<div>
				<div class="text-xs text-[var(--color-muted-foreground)] mb-1">Updated</div>
				<div class="text-sm">{new Date(data.reservation.updatedAt).toLocaleString()}</div>
			</div>
		</div>

		<div class="grid grid-cols-2 gap-4 p-4">
			<div>
				<div class="text-xs text-[var(--color-muted-foreground)] mb-1">Lease</div>
				<div class="text-sm">
					{#if data.reservation.leaseSeconds}
						{data.reservation.leaseSeconds}s
						{#if data.reservation.expiresAt}
							<span class="text-[var(--color-muted-foreground)]">
								(expires {new Date(data.reservation.expiresAt).toLocaleString()})
							</span>
						{/if}
					{:else}
						<span class="text-[var(--color-muted-foreground)]">indefinite</span>
					{/if}
				</div>
			</div>
		</div>
	</div>

	<!-- Tunnel Health (VM only) -->
	{#if isVm}
		<div class="mt-6 rounded-lg border border-[var(--color-border)]">
			<div class="px-4 py-3 border-b border-[var(--color-border)]">
				<h2 class="text-sm font-medium">Tunnel Health</h2>
			</div>
			{#if health}
				<div class="grid grid-cols-4 gap-4 p-4">
					<div>
						<div class="text-xs text-[var(--color-muted-foreground)] mb-1">Status</div>
						<div class="flex items-center gap-2">
							<span class="h-2 w-2 rounded-full {health.alive ? 'bg-[var(--color-success)] animate-pulse' : 'bg-[var(--color-destructive)]'}"></span>
							<span class="text-sm font-medium {health.alive ? 'text-[var(--color-success)]' : 'text-[var(--color-destructive)]'}">
								{health.alive ? 'Alive' : 'Dead'}
							</span>
						</div>
					</div>
					<div>
						<div class="text-xs text-[var(--color-muted-foreground)] mb-1">Uptime</div>
						<div class="text-sm font-mono">{formatUptime(health.uptimeSeconds)}</div>
					</div>
					<div>
						<div class="text-xs text-[var(--color-muted-foreground)] mb-1">Reconnects</div>
						<div class="text-sm font-mono {health.reconnectCount > 0 ? 'text-[var(--color-warning)]' : ''}">
							{health.reconnectCount}
						</div>
					</div>
					<div>
						<div class="text-xs text-[var(--color-muted-foreground)] mb-1">Last Check</div>
						<div class="text-sm">
							{health.lastCheck ? new Date(health.lastCheck).toLocaleTimeString() : 'n/a'}
						</div>
					</div>
				</div>
			{:else}
				<div class="p-4 text-sm text-[var(--color-muted-foreground)]">
					No health data available
				</div>
			{/if}
		</div>
	{:else}
		<div class="mt-6 rounded-lg border border-[var(--color-border)] p-4">
			<p class="text-sm text-[var(--color-muted-foreground)]">
				Direct host port hold — no tunnel
			</p>
		</div>
	{/if}
</div>
