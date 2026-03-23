<script lang="ts">
	import '../app.css';
	import { ModeWatcher, toggleMode, mode } from 'mode-watcher';

	let { children, data } = $props();

	const currentMode = $derived(mode.current);
</script>

<ModeWatcher defaultMode="dark" />

<div class="min-h-screen bg-[var(--color-background)] text-[var(--color-foreground)]">
	<!-- Header -->
	<header class="border-b border-[var(--color-border)] px-6 py-3">
		<div class="flex items-center justify-between">
			<div class="flex items-center gap-6">
				<a href="/" class="flex items-center gap-2 text-lg font-bold tracking-tight">
					<span class="text-[var(--color-primary)]">⬡</span>
					Port Authority
				</a>
				<nav class="flex items-center gap-4 text-sm">
					<a href="/" class="text-[var(--color-muted-foreground)] hover:text-[var(--color-foreground)] transition-colors">
						Dashboard
					</a>
					<a href="/reserve" class="text-[var(--color-muted-foreground)] hover:text-[var(--color-foreground)] transition-colors">
						Reserve
					</a>
				</nav>
			</div>
			<div class="flex items-center gap-4">
				<!-- Daemon status -->
				<div class="flex items-center gap-2 text-xs text-[var(--color-muted-foreground)]">
					{#if data.daemonConnected}
						<span class="h-2 w-2 rounded-full bg-[var(--color-success)] animate-pulse"></span>
						portd
					{:else}
						<span class="h-2 w-2 rounded-full bg-[var(--color-destructive)]"></span>
						<span class="text-[var(--color-destructive)]">portd offline</span>
					{/if}
				</div>
				<!-- Dark mode toggle -->
				<button
					onclick={toggleMode}
					class="text-[var(--color-muted-foreground)] hover:text-[var(--color-foreground)] text-sm transition-colors"
					aria-label="Toggle dark mode"
				>
					{currentMode === 'dark' ? '☀' : '☾'}
				</button>
			</div>
		</div>
	</header>

	<!-- Main content -->
	<main class="px-6 py-6">
		{@render children()}
	</main>
</div>
