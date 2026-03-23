<script lang="ts">
	import { goto } from '$app/navigation';

	let ownerType = $state<'host' | 'vm'>('host');
	let vmName = $state('');
	let serviceName = $state('');
	let targetHost = $state('127.0.0.1');
	let targetPort = $state('');
	let preferredPort = $state('');
	let exactOnly = $state(false);
	let leaseEnabled = $state(false);
	let leaseSeconds = $state('3600');
	let submitting = $state(false);
	let error = $state<string | null>(null);

	function assembledOwner(): string {
		if (ownerType === 'host') return `host:${serviceName}`;
		return `vm:${vmName}:${serviceName}`;
	}

	async function handleSubmit(e: Event) {
		e.preventDefault();
		error = null;
		submitting = true;

		try {
			const body: Record<string, unknown> = {
				owner: assembledOwner(),
				target_host: targetHost,
				target_port: parseInt(targetPort, 10),
				exact_only: exactOnly
			};

			if (preferredPort) {
				body.preferred_port = parseInt(preferredPort, 10);
			}
			if (leaseEnabled && leaseSeconds) {
				body.lease_seconds = parseInt(leaseSeconds, 10);
			}

			const res = await fetch('/api/reservations', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify(body)
			});

			const data = await res.json();
			if (!res.ok) {
				error = data.error || 'Failed to reserve port';
				return;
			}

			goto(`/?reserved=${data.assigned_port}`);
		} catch (err) {
			error = err instanceof Error ? err.message : 'An error occurred';
		} finally {
			submitting = false;
		}
	}
</script>

<div class="max-w-xl">
	<div class="mb-6">
		<h1 class="text-xl font-bold">Reserve Port</h1>
		<p class="text-sm text-[var(--color-muted-foreground)]">
			Allocate a port on the host for a service
		</p>
	</div>

	{#if error}
		<div class="mb-4 rounded-md border border-red-500/20 bg-red-500/10 p-3 text-sm text-[var(--color-destructive)]">
			{error}
		</div>
	{/if}

	<form onsubmit={handleSubmit} class="space-y-6">
		<!-- Owner Type -->
		<div>
			<label class="block text-sm font-medium mb-2">Owner Type</label>
			<div class="flex gap-2">
				<button
					type="button"
					class="rounded-md px-4 py-2 text-sm font-medium border transition-colors {ownerType === 'host'
						? 'bg-[var(--color-primary)] text-[var(--color-primary-foreground)] border-[var(--color-primary)]'
						: 'bg-[var(--color-muted)] text-[var(--color-muted-foreground)] border-[var(--color-border)] hover:text-[var(--color-foreground)]'}"
					onclick={() => (ownerType = 'host')}
				>
					Host
				</button>
				<button
					type="button"
					class="rounded-md px-4 py-2 text-sm font-medium border transition-colors {ownerType === 'vm'
						? 'bg-[var(--color-primary)] text-[var(--color-primary-foreground)] border-[var(--color-primary)]'
						: 'bg-[var(--color-muted)] text-[var(--color-muted-foreground)] border-[var(--color-border)] hover:text-[var(--color-foreground)]'}"
					onclick={() => (ownerType = 'vm')}
				>
					VM
				</button>
			</div>
		</div>

		<!-- VM Name (conditional) -->
		{#if ownerType === 'vm'}
			<div>
				<label for="vmName" class="block text-sm font-medium mb-1">VM Name</label>
				<input
					id="vmName"
					type="text"
					bind:value={vmName}
					placeholder="e.g. smith"
					required
					class="w-full rounded-md border border-[var(--color-border)] bg-[var(--color-muted)] px-3 py-2 text-sm placeholder:text-[var(--color-muted-foreground)] focus:outline-none focus:ring-2 focus:ring-[var(--color-primary)]"
				/>
				<p class="mt-1 text-xs text-[var(--color-muted-foreground)]">Must match a VM in ssh.toml</p>
			</div>
		{/if}

		<!-- Service Name -->
		<div>
			<label for="serviceName" class="block text-sm font-medium mb-1">Service Name</label>
			<input
				id="serviceName"
				type="text"
				bind:value={serviceName}
				placeholder="e.g. web, api, db"
				required
				class="w-full rounded-md border border-[var(--color-border)] bg-[var(--color-muted)] px-3 py-2 text-sm placeholder:text-[var(--color-muted-foreground)] focus:outline-none focus:ring-2 focus:ring-[var(--color-primary)]"
			/>
			<p class="mt-1 text-xs text-[var(--color-muted-foreground)]">
				Owner will be: <code class="text-[var(--color-foreground)]">{assembledOwner() || '...'}</code>
			</p>
		</div>

		<!-- Target -->
		<div class="grid grid-cols-2 gap-4">
			<div>
				<label for="targetHost" class="block text-sm font-medium mb-1">Target Host</label>
				<input
					id="targetHost"
					type="text"
					bind:value={targetHost}
					placeholder="127.0.0.1"
					required
					class="w-full rounded-md border border-[var(--color-border)] bg-[var(--color-muted)] px-3 py-2 text-sm placeholder:text-[var(--color-muted-foreground)] focus:outline-none focus:ring-2 focus:ring-[var(--color-primary)]"
				/>
			</div>
			<div>
				<label for="targetPort" class="block text-sm font-medium mb-1">Target Port</label>
				<input
					id="targetPort"
					type="number"
					bind:value={targetPort}
					placeholder="8080"
					required
					min="1"
					max="65535"
					class="w-full rounded-md border border-[var(--color-border)] bg-[var(--color-muted)] px-3 py-2 text-sm placeholder:text-[var(--color-muted-foreground)] focus:outline-none focus:ring-2 focus:ring-[var(--color-primary)]"
				/>
			</div>
		</div>

		<!-- Preferred Port -->
		<div class="grid grid-cols-2 gap-4">
			<div>
				<label for="preferredPort" class="block text-sm font-medium mb-1">Preferred Port
					<span class="text-[var(--color-muted-foreground)] font-normal">(optional)</span>
				</label>
				<input
					id="preferredPort"
					type="number"
					bind:value={preferredPort}
					placeholder="auto"
					min="1"
					max="65535"
					class="w-full rounded-md border border-[var(--color-border)] bg-[var(--color-muted)] px-3 py-2 text-sm placeholder:text-[var(--color-muted-foreground)] focus:outline-none focus:ring-2 focus:ring-[var(--color-primary)]"
				/>
			</div>
			<div class="flex items-end pb-2">
				<label class="flex items-center gap-2 text-sm">
					<input type="checkbox" bind:checked={exactOnly} class="rounded" />
					Exact only
				</label>
			</div>
		</div>

		<!-- Lease -->
		<div>
			<label class="flex items-center gap-2 text-sm font-medium mb-2">
				<input type="checkbox" bind:checked={leaseEnabled} class="rounded" />
				Set lease duration
			</label>
			{#if leaseEnabled}
				<input
					type="number"
					bind:value={leaseSeconds}
					placeholder="3600"
					min="1"
					class="w-full rounded-md border border-[var(--color-border)] bg-[var(--color-muted)] px-3 py-2 text-sm placeholder:text-[var(--color-muted-foreground)] focus:outline-none focus:ring-2 focus:ring-[var(--color-primary)]"
				/>
				<p class="mt-1 text-xs text-[var(--color-muted-foreground)]">Seconds until auto-release</p>
			{/if}
		</div>

		<!-- Submit -->
		<div class="flex gap-3">
			<button
				type="submit"
				disabled={submitting}
				class="rounded-md bg-[var(--color-primary)] px-6 py-2 text-sm font-medium text-[var(--color-primary-foreground)] hover:opacity-90 transition-opacity disabled:opacity-50"
			>
				{submitting ? 'Reserving...' : 'Reserve'}
			</button>
			<a
				href="/"
				class="rounded-md border border-[var(--color-border)] px-6 py-2 text-sm font-medium text-[var(--color-muted-foreground)] hover:text-[var(--color-foreground)] transition-colors"
			>
				Cancel
			</a>
		</div>
	</form>
</div>
