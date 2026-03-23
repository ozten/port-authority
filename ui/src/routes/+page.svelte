<script lang="ts">
	let { data } = $props();

	const stateColors: Record<string, string> = {
		pending: 'text-[var(--color-warning)] bg-yellow-500/10 border-yellow-500/20',
		active: 'text-[var(--color-success)] bg-green-500/10 border-green-500/20',
		failed: 'text-[var(--color-destructive)] bg-red-500/10 border-red-500/20',
		released: 'text-[var(--color-muted-foreground)] bg-zinc-500/10 border-zinc-500/20'
	};

	const stateIcons: Record<string, string> = {
		pending: '◷',
		active: '●',
		failed: '✕',
		released: '○'
	};

	function relativeTime(iso: string): string {
		const diff = Date.now() - new Date(iso).getTime();
		const seconds = Math.floor(diff / 1000);
		if (seconds < 60) return `${seconds}s ago`;
		const minutes = Math.floor(seconds / 60);
		if (minutes < 60) return `${minutes}m ago`;
		const hours = Math.floor(minutes / 60);
		if (hours < 24) return `${hours}h ago`;
		const days = Math.floor(hours / 24);
		return `${days}d ago`;
	}
</script>

<div>
	<div class="mb-6 flex items-center justify-between">
		<div>
			<h1 class="text-xl font-bold">Port Reservations</h1>
			<p class="text-sm text-[var(--color-muted-foreground)]">
				{data.reservations.length} active reservation{data.reservations.length !== 1 ? 's' : ''}
			</p>
		</div>
		<a
			href="/reserve"
			class="rounded-md bg-[var(--color-primary)] px-4 py-2 text-sm font-medium text-[var(--color-primary-foreground)] hover:opacity-90 transition-opacity"
		>
			+ Reserve Port
		</a>
	</div>

	{#if data.error}
		<div class="rounded-md border border-red-500/20 bg-red-500/10 p-4 text-sm text-[var(--color-destructive)]">
			{data.error}
		</div>
	{:else if data.reservations.length === 0}
		<!-- Empty state -->
		<div class="flex flex-col items-center justify-center rounded-lg border border-dashed border-[var(--color-border)] py-16">
			<div class="text-4xl mb-4">⬡</div>
			<h2 class="text-lg font-medium mb-2">No port reservations</h2>
			<p class="text-sm text-[var(--color-muted-foreground)] mb-4">
				Reserve a port to get started
			</p>
			<a
				href="/reserve"
				class="rounded-md bg-[var(--color-primary)] px-4 py-2 text-sm font-medium text-[var(--color-primary-foreground)] hover:opacity-90 transition-opacity"
			>
				Reserve Port
			</a>
		</div>
	{:else}
		<!-- Reservation table -->
		<div class="overflow-x-auto rounded-lg border border-[var(--color-border)]">
			<table class="w-full text-sm">
				<thead>
					<tr class="border-b border-[var(--color-border)] bg-[var(--color-muted)]">
						<th class="px-4 py-3 text-left font-medium text-[var(--color-muted-foreground)]">Port</th>
						<th class="px-4 py-3 text-left font-medium text-[var(--color-muted-foreground)]">Owner</th>
						<th class="px-4 py-3 text-left font-medium text-[var(--color-muted-foreground)]">Target</th>
						<th class="px-4 py-3 text-left font-medium text-[var(--color-muted-foreground)]">State</th>
						<th class="px-4 py-3 text-left font-medium text-[var(--color-muted-foreground)]">Lease</th>
						<th class="px-4 py-3 text-left font-medium text-[var(--color-muted-foreground)]">Created</th>
						<th class="px-4 py-3 text-right font-medium text-[var(--color-muted-foreground)]">Actions</th>
					</tr>
				</thead>
				<tbody>
					{#each data.reservations as reservation (reservation.id)}
						<tr class="border-b border-[var(--color-border)] hover:bg-[var(--color-muted)]/50 transition-colors">
							<!-- Port -->
							<td class="px-4 py-3">
								<span class="font-mono font-bold text-base">
									{reservation.assignedPort}
								</span>
								{#if reservation.requestedPort && reservation.requestedPort !== reservation.assignedPort}
									<span class="text-xs text-[var(--color-muted-foreground)] ml-1">
										(req {reservation.requestedPort})
									</span>
								{/if}
							</td>
							<!-- Owner -->
							<td class="px-4 py-3">
								<div class="flex items-center gap-2">
									<span
										class="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-bold uppercase tracking-wider border {reservation.ownerType === 'vm'
											? 'text-[var(--color-primary)] bg-indigo-500/10 border-indigo-500/20'
											: 'text-[var(--color-muted-foreground)] bg-zinc-500/10 border-zinc-500/20'}"
									>
										{reservation.ownerType === 'vm' ? 'VM' : 'HOST'}
									</span>
									<span>{reservation.ownerLabel}</span>
								</div>
							</td>
							<!-- Target -->
							<td class="px-4 py-3 font-mono text-[var(--color-muted-foreground)]">
								{reservation.targetHost}:{reservation.targetPort}
							</td>
							<!-- State -->
							<td class="px-4 py-3">
								<span
									class="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium border {stateColors[reservation.state] ?? stateColors.pending}"
								>
									<span>{stateIcons[reservation.state] ?? '?'}</span>
									{reservation.state}
								</span>
							</td>
							<!-- Lease -->
							<td class="px-4 py-3 text-[var(--color-muted-foreground)]">
								{#if reservation.leaseSeconds}
									<span class="text-xs">{reservation.leaseSeconds}s</span>
								{:else}
									<span class="text-xs">indefinite</span>
								{/if}
							</td>
							<!-- Created -->
							<td class="px-4 py-3 text-[var(--color-muted-foreground)]" title={reservation.createdAt}>
								<span class="text-xs">{relativeTime(reservation.createdAt)}</span>
							</td>
							<!-- Actions -->
							<td class="px-4 py-3 text-right">
								<div class="flex items-center justify-end gap-2">
									<a
										href="/reservations/{reservation.id}"
										class="rounded px-2 py-1 text-xs text-[var(--color-muted-foreground)] hover:text-[var(--color-foreground)] hover:bg-[var(--color-muted)] transition-colors"
									>
										inspect
									</a>
									<button
										class="rounded px-2 py-1 text-xs text-[var(--color-destructive)] hover:bg-red-500/10 transition-colors"
										onclick={() => {
											if (confirm('Release this reservation?')) {
												fetch('/api/reservations', {
													method: 'DELETE',
													headers: { 'Content-Type': 'application/json' },
													body: JSON.stringify({ reservation_id: reservation.id })
												}).then(() => location.reload());
											}
										}}
									>
										release
									</button>
								</div>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</div>
