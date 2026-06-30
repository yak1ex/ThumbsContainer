<script lang="ts">
  import { tick } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";

  type RegisterResult = {
    scanned_files: number;
    inserted_files: number;
    updated_paths: number;
    skipped_files: number;
  };

  type FileRecord = {
    id: number;
    path: string;
    filename: string;
    hash: string;
    size: number;
    created_at: string;
  };

  let folderPath = $state("");
  let statusMessage = $state("Initializing...");
  let loading = $state(false);
  let lastResult = $state<RegisterResult | null>(null);
  let recentFiles = $state<FileRecord[]>([]);

  async function loadRecentFiles() {
    recentFiles = await invoke<FileRecord[]>("list_recent_files", { limit: 25 });
  }

  async function initialize() {
    try {
      await invoke<string>("init_database");
      statusMessage = "Database is ready.";
      await loadRecentFiles();
    } catch (error) {
      statusMessage = `Initialization failed: ${String(error)}`;
    }
  }

  async function chooseFolder() {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Select folder to register"
      });

      if (typeof selected === "string") {
        folderPath = selected;
        statusMessage = "Folder selected.";
      }
    } catch (error) {
      statusMessage = `Browse failed: ${String(error)}`;
    }
  }

  async function registerFolder(event: Event) {
    event.preventDefault();
    if (!folderPath.trim() || loading) {
      return;
    }

    loading = true;
    statusMessage = "Registering files...";
    lastResult = null;
    await tick();

    try {
      lastResult = await invoke<RegisterResult>("register_folder", { folderPath });
      statusMessage = "Registration finished.";
      await loadRecentFiles();
    } catch (error) {
      statusMessage = `Registration failed: ${String(error)}`;
    } finally {
      loading = false;
    }
  }

  initialize();
</script>

<main class="page">
  <section class="panel hero">
    <h1>ThumbsContainer</h1>
    <p class="subtitle">First implementation slice: folder registration and metadata storage.</p>
    <p class="status">{statusMessage}</p>
  </section>

  <section class="panel">
    <h2>Register Folder</h2>
    <form class="register-form" onsubmit={registerFolder}>
      <input
        id="folder-input"
        placeholder="C:\\path\\to\\media"
        bind:value={folderPath}
        disabled={loading}
      />
      <div class="actions">
        <button type="button" class="secondary" onclick={chooseFolder} disabled={loading}>
          Browse
        </button>
        <button type="submit" disabled={loading || !folderPath.trim()}>
          {loading ? "Registering..." : "Register"}
        </button>
      </div>
    </form>

    {#if lastResult}
      <div class="result-grid">
        <p>Scanned: <strong>{lastResult.scanned_files}</strong></p>
        <p>Inserted: <strong>{lastResult.inserted_files}</strong></p>
        <p>Moved updates: <strong>{lastResult.updated_paths}</strong></p>
        <p>Skipped: <strong>{lastResult.skipped_files}</strong></p>
      </div>
    {/if}
  </section>

  <section class="panel">
    <h2>Recent Files</h2>
    {#if recentFiles.length === 0}
      <p class="muted">No records yet.</p>
    {:else}
      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>ID</th>
              <th>Name</th>
              <th>Size</th>
              <th>Hash</th>
            </tr>
          </thead>
          <tbody>
            {#each recentFiles as file}
              <tr>
                <td>{file.id}</td>
                <td>
                  <div class="name">{file.filename}</div>
                  <div class="path">{file.path}</div>
                </td>
                <td>{file.size}</td>
                <td class="hash">{file.hash.slice(0, 16)}...</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </section>
</main>

<style>
  :global(body) {
    margin: 0;
    color: #15202b;
    font-family: "Source Sans 3", "Segoe UI", sans-serif;
    background:
      radial-gradient(circle at 10% 10%, rgba(255, 209, 102, 0.22), transparent 30%),
      radial-gradient(circle at 90% 20%, rgba(6, 214, 160, 0.2), transparent 34%),
      linear-gradient(140deg, #f7f3e9 0%, #e9f0ff 50%, #f3f8f4 100%);
  }

  .page {
    max-width: 1100px;
    margin: 0 auto;
    padding: 2rem 1rem 3rem;
    display: grid;
    gap: 1rem;
  }

  .panel {
    background: rgba(255, 255, 255, 0.82);
    border: 1px solid rgba(26, 26, 26, 0.08);
    border-radius: 14px;
    padding: 1rem;
    backdrop-filter: blur(6px);
    box-shadow: 0 12px 28px rgba(24, 48, 80, 0.08);
  }

  .hero h1 {
    margin: 0;
    font-size: clamp(1.8rem, 4vw, 2.5rem);
    font-family: "Merriweather", Georgia, serif;
  }

  .subtitle {
    margin: 0.25rem 0;
    color: #374151;
  }

  .status {
    margin: 0.5rem 0 0;
    color: #0a5f5a;
    font-weight: 600;
  }

  h2 {
    margin: 0 0 0.6rem;
    font-size: 1.15rem;
  }

  .register-form {
    display: grid;
    gap: 0.65rem;
  }

  .actions {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  input,
  button {
    border-radius: 10px;
    border: 1px solid #d5dce8;
    padding: 0.65rem 0.75rem;
    font: inherit;
  }

  input {
    width: 100%;
    background: #fcfdff;
  }

  button {
    cursor: pointer;
    background: #0c6b58;
    color: #fff;
    border-color: #0c6b58;
    font-weight: 600;
  }

  button.secondary {
    background: #fff;
    color: #1f2937;
    border-color: #cfd8e3;
  }

  button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .result-grid {
    margin-top: 0.75rem;
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(120px, 1fr));
    gap: 0.5rem;
  }

  .result-grid p {
    margin: 0;
    padding: 0.5rem;
    border-radius: 9px;
    background: #eef7ff;
  }

  .table-wrap {
    overflow: auto;
    border-radius: 12px;
    border: 1px solid #e1e7f0;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    min-width: 680px;
    font-size: 0.95rem;
  }

  th,
  td {
    text-align: left;
    padding: 0.55rem 0.65rem;
    border-bottom: 1px solid #edf1f7;
    vertical-align: top;
  }

  th {
    background: #f8fafc;
    font-size: 0.85rem;
    text-transform: uppercase;
    letter-spacing: 0.03em;
    color: #334155;
  }

  .name {
    font-weight: 600;
  }

  .path,
  .hash,
  .muted {
    color: #64748b;
    font-size: 0.85rem;
  }

  @media (max-width: 640px) {
    .page {
      padding: 1rem 0.75rem 2rem;
    }

    .panel {
      padding: 0.85rem;
    }
  }
</style>
