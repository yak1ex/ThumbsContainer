<script lang="ts">
  import { onMount, tick } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";

  type RegisterResult = {
    scanned_files: number;
    inserted_files: number;
    updated_paths: number;
    skipped_files: number;
    created_containers: number;
    updated_containers: number;
    created_group_containers: number;
    updated_group_containers: number;
    created_thumbnails: number;
    updated_thumbnails: number;
    canceled: boolean;
  };

  type FileRecord = {
    id: number;
    path: string;
    filename: string;
    hash: string;
    size: number;
    created_at: string;
    thumbnail_path: string | null;
    thumbnail_data_url: string | null;
  };

  type FileClassification = {
    tags: string[];
    rating: number | null;
  };

  type ContainerRecord = {
    id: number;
    container_type: string;
    display_name: string;
    source_path: string;
    updated_at: string;
    child_count: number;
  };

  type ContainerChildRecord = {
    id: number;
    container_type: string;
    display_name: string;
    source_path: string;
  };

  type ContainerThumbnailRecord = {
    slot_index: number;
    thumbnail_path: string;
    thumbnail_data_url: string | null;
  };

  type ArchiveBackfillResult = {
    container_id: number;
    generated_slots: number;
    updated_slots: number;
    updated_file_thumbnail: boolean;
    skipped_reason: string | null;
  };

  type ThumbnailMaintenanceResult = {
    dry_run: boolean;
    referenced_thumbnail_files: number;
    missing_file_thumbnail_records: number;
    missing_container_thumbnail_records: number;
    orphaned_cache_files: number;
    removed_file_thumbnail_records: number;
    removed_container_thumbnail_records: number;
    removed_orphaned_cache_files: number;
  };

  type RegisterProgress = {
    folder_path: string;
    total_files: number;
    scanned_files: number;
    inserted_files: number;
    updated_paths: number;
    skipped_files: number;
    current_file_path: string | null;
    current_file_size_bytes: number;
    current_file_processed_bytes: number;
    canceled: boolean;
    done: boolean;
  };

  let folderPath = $state("");
  let statusMessage = $state("Initializing...");
  let loading = $state(false);
  let searching = $state(false);
  let lastResult = $state<RegisterResult | null>(null);
  let recentFiles = $state<FileRecord[]>([]);
  let recentContainers = $state<ContainerRecord[]>([]);
  let expandedContainers = $state<Record<number, boolean>>({});
  let containerChildren = $state<Record<number, ContainerChildRecord[]>>({});
  let loadingChildren = $state<Record<number, boolean>>({});
  let containerThumbs = $state<Record<number, ContainerThumbnailRecord[]>>({});
  let loadingThumbs = $state<Record<number, boolean>>({});
  let backfillingThumbs = $state<Record<number, boolean>>({});
  let maintenanceBusy = $state(false);
  let lastMaintenance = $state<ThumbnailMaintenanceResult | null>(null);
  let searchResults = $state<FileRecord[]>([]);
  let searchContainerResults = $state<ContainerRecord[]>([]);
  let focusedContainerId = $state<number | null>(null);
  let searchPath = $state("");
  let searchFilename = $state("");
  let searchTag = $state("");
  let searchMinRating = $state("");
  let searchIncludeContainers = $state(true);
  let searchRan = $state(false);
  let selectedFileId = $state<number | null>(null);
  let classTagsInput = $state("");
  let classRating = $state("");
  let classBusy = $state(false);
  let progress = $state<RegisterProgress | null>(null);
  let cancelRequested = $state(false);
  let overallStartMs = $state<number | null>(null);
  let overallEtaSeconds = $state<number | null>(null);
  let currentFileEtaPath = $state<string | null>(null);
  let currentFileEtaStartMs = $state<number | null>(null);
  let currentFileEtaStartBytes = $state(0);
  let currentFileEtaSeconds = $state<number | null>(null);

  const progressPercent = $derived.by(() => {
    if (!progress || progress.total_files === 0) {
      return 0;
    }
    return Math.min(100, Math.floor((progress.scanned_files / progress.total_files) * 100));
  });

  const currentFileProgressPercent = $derived.by(() => {
    if (!progress || progress.current_file_size_bytes <= 0) {
      return 0;
    }
    return Math.min(
      100,
      Math.floor((progress.current_file_processed_bytes / progress.current_file_size_bytes) * 100)
    );
  });

  function formatBytes(bytes: number): string {
    if (!Number.isFinite(bytes) || bytes < 0) {
      return "0 B";
    }

    const units = ["B", "KB", "MB", "GB", "TB"];
    let value = bytes;
    let idx = 0;
    while (value >= 1024 && idx < units.length - 1) {
      value /= 1024;
      idx += 1;
    }

    const decimals = idx === 0 ? 0 : 1;
    return `${value.toFixed(decimals)} ${units[idx]}`;
  }

  function formatEta(seconds: number | null): string {
    if (seconds === null || !Number.isFinite(seconds) || seconds < 0) {
      return "--";
    }

    const rounded = Math.max(0, Math.floor(seconds));
    const mins = Math.floor(rounded / 60);
    const secs = rounded % 60;

    if (mins <= 0) {
      return `${secs}s`;
    }
    if (mins < 60) {
      return `${mins}m ${secs}s`;
    }

    const hours = Math.floor(mins / 60);
    const remMins = mins % 60;
    return `${hours}h ${remMins}m`;
  }

  function displayThumbnailUrl(file: FileRecord): string | null {
    if (file.thumbnail_data_url) {
      return file.thumbnail_data_url;
    }
    if (file.thumbnail_path) {
      return file.thumbnail_path;
    }
    return null;
  }

  function hasThumbnail(file: FileRecord): boolean {
    return displayThumbnailUrl(file) !== null;
  }

  async function loadRecentFiles() {
    recentFiles = await invoke<FileRecord[]>("list_recent_files", { limit: 25 });
  }

  async function loadContainerThumbnails(containerId: number) {
    if (containerThumbs[containerId]) {
      return;
    }

    loadingThumbs = { ...loadingThumbs, [containerId]: true };
    try {
      const thumbs = await invoke<ContainerThumbnailRecord[]>("list_container_thumbnails", {
        containerId,
        limit: 16
      });
      containerThumbs = { ...containerThumbs, [containerId]: thumbs };
    } catch {
      containerThumbs = { ...containerThumbs, [containerId]: [] };
    } finally {
      loadingThumbs = { ...loadingThumbs, [containerId]: false };
    }
  }

  async function loadRecentContainers() {
    recentContainers = await invoke<ContainerRecord[]>("list_recent_containers", { limit: 25 });

    const ids = recentContainers.map((container) => container.id);
    await Promise.all(ids.map((id) => loadContainerThumbnails(id)));
  }

  async function toggleContainerChildren(containerId: number) {
    const isExpanded = !!expandedContainers[containerId];

    if (isExpanded) {
      expandedContainers = { ...expandedContainers, [containerId]: false };
      return;
    }

    expandedContainers = { ...expandedContainers, [containerId]: true };

    if (containerChildren[containerId]) {
      return;
    }

    loadingChildren = { ...loadingChildren, [containerId]: true };
    if (!containerThumbs[containerId]) {
      loadingThumbs = { ...loadingThumbs, [containerId]: true };
    }
    try {
      const children = await invoke<ContainerChildRecord[]>("list_container_children", {
        containerId
      });
      const thumbs = containerThumbs[containerId]
        ? containerThumbs[containerId]
        : await invoke<ContainerThumbnailRecord[]>("list_container_thumbnails", {
            containerId,
            limit: 16
          });
      containerChildren = { ...containerChildren, [containerId]: children };
      containerThumbs = { ...containerThumbs, [containerId]: thumbs };
    } catch (error) {
      statusMessage = `Failed to load children for container ${containerId}: ${String(error)}`;
      containerChildren = { ...containerChildren, [containerId]: [] };
      containerThumbs = { ...containerThumbs, [containerId]: [] };
    } finally {
      loadingChildren = { ...loadingChildren, [containerId]: false };
      loadingThumbs = { ...loadingThumbs, [containerId]: false };
    }
  }

  async function backfillArchiveContainer(container: ContainerRecord) {
    if (container.container_type !== "archive" || backfillingThumbs[container.id]) {
      return;
    }

    backfillingThumbs = { ...backfillingThumbs, [container.id]: true };
    statusMessage = `Backfilling archive thumbnails for container ${container.id}...`;

    try {
      const result = await invoke<ArchiveBackfillResult>("backfill_archive_container_thumbnails", {
        containerId: container.id
      });

      const refreshed = await invoke<ContainerThumbnailRecord[]>("list_container_thumbnails", {
        containerId: container.id,
        limit: 16
      });
      containerThumbs = { ...containerThumbs, [container.id]: refreshed };

      if (result.skipped_reason) {
        statusMessage = `Archive backfill skipped for container ${container.id}: ${result.skipped_reason}`;
      } else {
        statusMessage = `Archive backfill finished for container ${container.id}. Generated ${result.generated_slots} slot(s), updated ${result.updated_slots} slot(s).`;
      }

      await loadRecentFiles();
    } catch (error) {
      statusMessage = `Archive backfill failed for container ${container.id}: ${String(error)}`;
    } finally {
      backfillingThumbs = { ...backfillingThumbs, [container.id]: false };
    }
  }

  function describeMaintenanceResult(result: ThumbnailMaintenanceResult): string {
    const modeLabel = result.dry_run ? "Inspection" : "Cleanup";
    return `${modeLabel} finished. Referenced ${result.referenced_thumbnail_files} file(s), missing file records ${result.missing_file_thumbnail_records}, missing container records ${result.missing_container_thumbnail_records}, orphaned cache files ${result.orphaned_cache_files}. Removed file records ${result.removed_file_thumbnail_records}, removed container records ${result.removed_container_thumbnail_records}, removed orphan files ${result.removed_orphaned_cache_files}.`;
  }

  async function inspectThumbnailCache() {
    if (maintenanceBusy) {
      return;
    }

    maintenanceBusy = true;
    statusMessage = "Inspecting thumbnail cache...";

    try {
      const result = await invoke<ThumbnailMaintenanceResult>("inspect_thumbnail_cache");
      lastMaintenance = result;
      statusMessage = describeMaintenanceResult(result);
    } catch (error) {
      statusMessage = `Thumbnail cache inspection failed: ${String(error)}`;
    } finally {
      maintenanceBusy = false;
    }
  }

  async function cleanupThumbnailCache() {
    if (maintenanceBusy) {
      return;
    }

    maintenanceBusy = true;
    statusMessage = "Cleaning thumbnail cache...";

    try {
      const result = await invoke<ThumbnailMaintenanceResult>("cleanup_thumbnail_cache");
      lastMaintenance = result;
      statusMessage = describeMaintenanceResult(result);
      await loadRecentFiles();
      await loadRecentContainers();
    } catch (error) {
      statusMessage = `Thumbnail cache cleanup failed: ${String(error)}`;
    } finally {
      maintenanceBusy = false;
    }
  }

  async function openContainerInRecent(container: ContainerRecord) {
    if (!recentContainers.some((v) => v.id === container.id)) {
      recentContainers = [container, ...recentContainers];
      await loadContainerThumbnails(container.id);
    }

    if (container.child_count > 0 && !expandedContainers[container.id]) {
      await toggleContainerChildren(container.id);
    } else if (container.child_count === 0) {
      expandedContainers = { ...expandedContainers, [container.id]: false };
    }

    focusedContainerId = container.id;
    statusMessage = `Opened container ${container.id} in Recent Containers.`;

    await tick();
    const row = document.getElementById(`recent-container-row-${container.id}`);
    row?.scrollIntoView({ behavior: "smooth", block: "center" });
  }

  async function initialize() {
    try {
      await invoke<string>("init_database");
      statusMessage = "Database is ready.";
      await loadRecentFiles();
      await loadRecentContainers();
    } catch (error) {
      statusMessage = `Initialization failed: ${String(error)}`;
    }
  }

  async function runSearch(event: Event) {
    event.preventDefault();
    if (searching) {
      return;
    }

    searching = true;
    searchRan = true;
    statusMessage = "Searching...";

    try {
      searchResults = await invoke<FileRecord[]>("search_files", {
        pathQuery: searchPath,
        filenameQuery: searchFilename,
        tagQuery: searchTag,
        minRating: searchMinRating.trim() ? Number(searchMinRating) : null,
        limit: 250
      });

      if (searchIncludeContainers) {
        searchContainerResults = await invoke<ContainerRecord[]>("search_containers", {
          pathQuery: searchPath,
          nameQuery: searchFilename,
          limit: 250
        });
      } else {
        searchContainerResults = [];
      }

      statusMessage = `Search finished. ${searchResults.length} file result(s), ${searchContainerResults.length} container result(s).`;
    } catch (error) {
      statusMessage = `Search failed: ${String(error)}`;
    } finally {
      searching = false;
    }
  }

  function clearSearch() {
    searchPath = "";
    searchFilename = "";
    searchTag = "";
    searchMinRating = "";
    searchResults = [];
    searchContainerResults = [];
    searchIncludeContainers = true;
    searchRan = false;
    statusMessage = "Search cleared.";
  }

  async function loadClassification(fileId: number) {
    if (classBusy) {
      return;
    }

    classBusy = true;
    statusMessage = `Loading classification for file ${fileId}...`;

    try {
      selectedFileId = fileId;
      const classification = await invoke<FileClassification>("get_file_classification", {
        fileId
      });
      classTagsInput = classification.tags.join(", ");
      classRating = classification.rating === null ? "" : String(classification.rating);
      statusMessage = `Classification loaded for file ${fileId}.`;
    } catch (error) {
      statusMessage = `Failed to load classification: ${String(error)}`;
    } finally {
      classBusy = false;
    }
  }

  async function saveClassification() {
    if (selectedFileId === null || classBusy) {
      return;
    }

    const tags = classTagsInput
      .split(",")
      .map((v) => v.trim())
      .filter((v) => v.length > 0);

    const rating = classRating.trim() === "" ? null : Number(classRating);
    if (rating !== null && (!Number.isInteger(rating) || rating < 1 || rating > 5)) {
      statusMessage = "Rating must be an integer from 1 to 5.";
      return;
    }

    classBusy = true;
    statusMessage = `Saving classification for file ${selectedFileId}...`;

    try {
      await invoke<string>("save_file_classification", {
        fileId: selectedFileId,
        tags,
        rating
      });
      statusMessage = `Classification saved for file ${selectedFileId}.`;
    } catch (error) {
      statusMessage = `Failed to save classification: ${String(error)}`;
    } finally {
      classBusy = false;
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
    progress = null;
    cancelRequested = false;
    overallStartMs = null;
    overallEtaSeconds = null;
    currentFileEtaPath = null;
    currentFileEtaStartMs = null;
    currentFileEtaStartBytes = 0;
    currentFileEtaSeconds = null;
    await tick();

    try {
      lastResult = await invoke<RegisterResult>("register_folder", { folderPath });
      statusMessage = lastResult.canceled ? "Registration canceled." : "Registration finished.";
      await loadRecentFiles();
      await loadRecentContainers();
    } catch (error) {
      statusMessage = `Registration failed: ${String(error)}`;
    } finally {
      loading = false;
    }
  }

  async function cancelRegistration() {
    if (!loading || cancelRequested) {
      return;
    }

    cancelRequested = true;
    statusMessage = "Cancel requested...";

    try {
      const accepted = await invoke<boolean>("cancel_register");
      if (!accepted) {
        statusMessage = "No active registration to cancel.";
      }
    } catch (error) {
      cancelRequested = false;
      statusMessage = `Cancel failed: ${String(error)}`;
    }
  }

  onMount(() => {
    let unlisten: null | (() => void) = null;

    void listen<RegisterProgress>("register-progress", (event) => {
      const payload = event.payload;
      if (payload.folder_path !== folderPath) {
        return;
      }

      progress = payload;
      if (!payload.done) {
        const now = Date.now();

        if (payload.scanned_files > 0 && payload.total_files > 0) {
          if (overallStartMs === null) {
            overallStartMs = now;
          }

          if (overallStartMs !== null) {
            const elapsedSec = (now - overallStartMs) / 1000;
            const processed = payload.scanned_files;
            const remaining = Math.max(0, payload.total_files - processed);
            if (elapsedSec > 0 && processed > 0 && remaining > 0) {
              const rate = processed / elapsedSec;
              overallEtaSeconds = rate > 0 ? remaining / rate : null;
            } else if (remaining === 0) {
              overallEtaSeconds = 0;
            }
          }
        }

        if (payload.current_file_path && payload.current_file_size_bytes > 0) {
          if (currentFileEtaPath !== payload.current_file_path) {
            currentFileEtaPath = payload.current_file_path;
            currentFileEtaStartMs = now;
            currentFileEtaStartBytes = payload.current_file_processed_bytes;
            currentFileEtaSeconds = null;
          } else if (currentFileEtaStartMs !== null) {
            const elapsedSec = (now - currentFileEtaStartMs) / 1000;
            const deltaBytes = Math.max(
              0,
              payload.current_file_processed_bytes - currentFileEtaStartBytes
            );
            const remainingBytes = Math.max(
              0,
              payload.current_file_size_bytes - payload.current_file_processed_bytes
            );
            if (elapsedSec > 0 && deltaBytes > 0 && remainingBytes > 0) {
              const rate = deltaBytes / elapsedSec;
              currentFileEtaSeconds = rate > 0 ? remainingBytes / rate : null;
            } else if (remainingBytes === 0) {
              currentFileEtaSeconds = 0;
            }
          }
        }

        if (payload.total_files > 0) {
          statusMessage = `Registering files... ${payload.scanned_files}/${payload.total_files} (${progressPercent}%)`;
        } else {
          statusMessage = `Registering files... ${payload.scanned_files}`;
        }
      } else if (payload.canceled) {
        statusMessage = `Registration canceled at ${payload.scanned_files}/${payload.total_files || "?"}.`;
        overallEtaSeconds = null;
        currentFileEtaSeconds = null;
      } else {
        overallEtaSeconds = 0;
        currentFileEtaSeconds = 0;
      }
    }).then((fn) => {
      unlisten = fn;
    });

    return () => {
      if (unlisten) {
        unlisten();
      }
    };
  });

  initialize();
</script>

<main class="page">
  <section class="panel hero">
    <h1>ThumbsContainer</h1>
    <p class="subtitle">First implementation slice: folder registration and metadata storage.</p>
    <p class="status">{statusMessage}</p>
  </section>

  <section class="panel">
    <h2>Search</h2>
    <form class="register-form" onsubmit={runSearch}>
      <input
        placeholder="Path contains (example: camera\\2026)"
        bind:value={searchPath}
        disabled={searching || loading}
      />
      <input
        placeholder="Filename contains (example: .mp4 or IMG_)"
        bind:value={searchFilename}
        disabled={searching || loading}
      />
      <input
        placeholder="Tag exact match (example: favorite)"
        bind:value={searchTag}
        disabled={searching || loading}
      />
      <input
        placeholder="Minimum rating (1-5)"
        bind:value={searchMinRating}
        disabled={searching || loading}
      />
      <label class="checkbox-row">
        <input type="checkbox" bind:checked={searchIncludeContainers} disabled={searching || loading} />
        Include containers in search target
      </label>
      <div class="actions">
        <button type="submit" disabled={searching || loading}>
          {searching ? "Searching..." : "Search"}
        </button>
        <button type="button" class="secondary" onclick={clearSearch} disabled={searching || loading}>
          Clear
        </button>
      </div>
    </form>

    {#if searchRan}
      {#if searchResults.length > 0}
        <h3>Files</h3>
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th>ID</th>
                <th>Name</th>
                <th>Size</th>
                <th>Hash</th>
                <th>Action</th>
              </tr>
            </thead>
            <tbody>
              {#each searchResults as file}
                <tr>
                  <td>{file.id}</td>
                  <td>
                    <div class="file-cell">
                      {#if hasThumbnail(file)}
                        <img class="thumb" src={displayThumbnailUrl(file) ?? undefined} alt="thumbnail" />
                      {:else}
                        <div class="thumb placeholder">No preview</div>
                      {/if}
                      <div>
                        <div class="name">{file.filename}</div>
                        <div class="path">{file.path}</div>
                      </div>
                    </div>
                  </td>
                  <td>{file.size}</td>
                  <td class="hash">{file.hash.slice(0, 16)}...</td>
                  <td>
                    <button type="button" class="secondary" onclick={() => loadClassification(file.id)} disabled={classBusy}>
                      Classify
                    </button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {:else}
        <p class="muted">No matching files.</p>
      {/if}

      {#if searchIncludeContainers}
        {#if searchContainerResults.length > 0}
          <h3>Containers</h3>
          <div class="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>ID</th>
                  <th>Type</th>
                  <th>Name</th>
                  <th>Source Path</th>
                  <th>Children</th>
                  <th>Action</th>
                </tr>
              </thead>
              <tbody>
                {#each searchContainerResults as container}
                  <tr>
                    <td>{container.id}</td>
                    <td>{container.container_type}</td>
                    <td>{container.display_name}</td>
                    <td class="path">{container.source_path}</td>
                    <td>{container.child_count}</td>
                    <td>
                      <button
                        type="button"
                        class="secondary"
                        onclick={() => openContainerInRecent(container)}
                        disabled={searching || loading}
                      >
                        Open in Recent
                      </button>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {:else}
          <p class="muted">No matching containers.</p>
        {/if}
      {/if}
    {/if}
  </section>

  <section class="panel">
    <h2>Classification</h2>
    <form class="register-form" onsubmit={(e) => e.preventDefault()}>
      <input value={selectedFileId === null ? "" : String(selectedFileId)} placeholder="File ID" disabled />
      <input
        placeholder="Tags (comma-separated: favorite, action, reference)"
        bind:value={classTagsInput}
        disabled={classBusy || selectedFileId === null}
      />
      <select bind:value={classRating} disabled={classBusy || selectedFileId === null}>
        <option value="">No rating</option>
        <option value="1">1</option>
        <option value="2">2</option>
        <option value="3">3</option>
        <option value="4">4</option>
        <option value="5">5</option>
      </select>
      <div class="actions">
        <button type="button" onclick={saveClassification} disabled={classBusy || selectedFileId === null}>
          {classBusy ? "Saving..." : "Save Classification"}
        </button>
      </div>
    </form>
    <p class="muted">
      Select a file from Search or Recent Files using the Classify button, then edit tags/rating here.
    </p>
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
        <button type="button" class="danger" onclick={cancelRegistration} disabled={!loading || cancelRequested}>
          {cancelRequested ? "Canceling..." : "Cancel"}
        </button>
      </div>
      <div class="actions">
        <button type="button" class="secondary" onclick={inspectThumbnailCache} disabled={loading || maintenanceBusy}>
          {maintenanceBusy ? "Working..." : "Inspect thumbnails"}
        </button>
        <button type="button" class="secondary" onclick={cleanupThumbnailCache} disabled={loading || maintenanceBusy}>
          {maintenanceBusy ? "Working..." : "Cleanup thumbnails"}
        </button>
      </div>
    </form>

    {#if loading && progress}
      <div class="progress-wrap">
        <div class="progress-label">
          <span>{progress.scanned_files}/{progress.total_files || "?"} scanned</span>
          <span>{progressPercent}% | ETA {formatEta(overallEtaSeconds)}</span>
        </div>
        <progress max="100" value={progressPercent}></progress>

        {#if progress.current_file_path}
          <div class="subprogress-wrap">
            <div class="progress-label">
              <span class="file-name" title={progress.current_file_path}>{progress.current_file_path.split(/[/\\]/).pop()}</span>
              <span>{currentFileProgressPercent}% | ETA {formatEta(currentFileEtaSeconds)}</span>
            </div>
            <div class="progress-label minor">
              <span>{formatBytes(progress.current_file_processed_bytes)} / {formatBytes(progress.current_file_size_bytes)}</span>
              <span>Current file</span>
            </div>
            <progress max="100" value={currentFileProgressPercent} class="subprogress"></progress>
          </div>
        {/if}
      </div>
    {/if}

    {#if lastResult}
      <div class="result-grid">
        <p>Scanned: <strong>{lastResult.scanned_files}</strong></p>
        <p>Inserted: <strong>{lastResult.inserted_files}</strong></p>
        <p>Moved updates: <strong>{lastResult.updated_paths}</strong></p>
        <p>Skipped: <strong>{lastResult.skipped_files}</strong></p>
        <p>Containers created: <strong>{lastResult.created_containers}</strong></p>
        <p>Containers updated: <strong>{lastResult.updated_containers}</strong></p>
        <p>Group containers created: <strong>{lastResult.created_group_containers}</strong></p>
        <p>Group containers updated: <strong>{lastResult.updated_group_containers}</strong></p>
        <p>Thumbnails created: <strong>{lastResult.created_thumbnails}</strong></p>
        <p>Thumbnails updated: <strong>{lastResult.updated_thumbnails}</strong></p>
      </div>
    {/if}

    {#if lastMaintenance}
      <div class="result-grid">
        <p>Referenced thumbs: <strong>{lastMaintenance.referenced_thumbnail_files}</strong></p>
        <p>Missing file records: <strong>{lastMaintenance.missing_file_thumbnail_records}</strong></p>
        <p>Missing container records: <strong>{lastMaintenance.missing_container_thumbnail_records}</strong></p>
        <p>Orphan cache files: <strong>{lastMaintenance.orphaned_cache_files}</strong></p>
        <p>Removed file records: <strong>{lastMaintenance.removed_file_thumbnail_records}</strong></p>
        <p>Removed container records: <strong>{lastMaintenance.removed_container_thumbnail_records}</strong></p>
        <p>Removed orphan files: <strong>{lastMaintenance.removed_orphaned_cache_files}</strong></p>
        <p>Mode: <strong>{lastMaintenance.dry_run ? "Inspect" : "Cleanup"}</strong></p>
      </div>
    {/if}
  </section>

  <section class="panel">
    <h2>Recent Containers</h2>
    {#if recentContainers.length === 0}
      <p class="muted">No containers yet.</p>
    {:else}
      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>ID</th>
              <th>Type</th>
              <th>Name</th>
              <th>Source Path</th>
              <th>Preview</th>
              <th>Action</th>
            </tr>
          </thead>
          <tbody>
            {#each recentContainers as container}
              <tr
                id={`recent-container-row-${container.id}`}
                class:focused-container-row={focusedContainerId === container.id}
              >
                <td>{container.id}</td>
                <td>{container.container_type}</td>
                <td>{container.display_name}</td>
                <td class="path">{container.source_path}</td>
                <td class="container-preview-cell">
                  {#if loadingThumbs[container.id]}
                    <p class="muted">Loading...</p>
                  {:else if (containerThumbs[container.id] || []).length > 0}
                    <div class="container-thumb-grid compact">
                      {#each containerThumbs[container.id] as thumb}
                        <div class="container-thumb-cell" title={`slot ${thumb.slot_index}`}>
                          {#if thumb.thumbnail_data_url}
                            <img class="container-thumb" src={thumb.thumbnail_data_url} alt={`thumb ${thumb.slot_index}`} />
                          {:else}
                            <div class="container-thumb placeholder">No preview</div>
                          {/if}
                        </div>
                      {/each}
                    </div>
                  {:else}
                    <p class="muted">No preview</p>
                  {/if}
                </td>
                <td>
                  <div class="container-actions">
                    <button
                      type="button"
                      class="secondary"
                      onclick={() => toggleContainerChildren(container.id)}
                      disabled={container.child_count === 0}
                    >
                      {expandedContainers[container.id] ? "Hide" : "Show"} ({container.child_count})
                    </button>
                    {#if container.container_type === "archive"}
                      <button
                        type="button"
                        class="secondary"
                        onclick={() => backfillArchiveContainer(container)}
                        disabled={!!backfillingThumbs[container.id]}
                      >
                        {backfillingThumbs[container.id] ? "Backfilling..." : "Backfill thumbs"}
                      </button>
                    {/if}
                  </div>
                </td>
              </tr>
              {#if expandedContainers[container.id]}
                <tr class="child-row">
                  <td colspan="6">
                    {#if loadingThumbs[container.id]}
                      <p class="muted">Loading thumbnails...</p>
                    {:else if (containerThumbs[container.id] || []).length > 0}
                      <div class="container-thumb-grid">
                        {#each containerThumbs[container.id] as thumb}
                          <div class="container-thumb-cell" title={`slot ${thumb.slot_index}`}>
                            {#if thumb.thumbnail_data_url}
                              <img class="container-thumb" src={thumb.thumbnail_data_url} alt={`thumb ${thumb.slot_index}`} />
                            {:else}
                              <div class="container-thumb placeholder">No preview</div>
                            {/if}
                          </div>
                        {/each}
                      </div>
                    {/if}

                    {#if loadingChildren[container.id]}
                      <p class="muted">Loading children...</p>
                    {:else if (containerChildren[container.id] || []).length === 0}
                      <p class="muted">No child containers.</p>
                    {:else}
                      <ul class="child-list">
                        {#each containerChildren[container.id] as child}
                          <li>
                            <span class="child-type">{child.container_type}</span>
                            <span class="child-name">{child.display_name}</span>
                            <span class="child-path">{child.source_path}</span>
                          </li>
                        {/each}
                      </ul>
                    {/if}
                  </td>
                </tr>
              {/if}
            {/each}
          </tbody>
        </table>
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
              <th>Action</th>
            </tr>
          </thead>
          <tbody>
            {#each recentFiles as file}
              <tr>
                <td>{file.id}</td>
                <td>
                    <div class="file-cell">
                      {#if hasThumbnail(file)}
                        <img class="thumb" src={displayThumbnailUrl(file) ?? undefined} alt="thumbnail" />
                      {:else}
                        <div class="thumb placeholder">No preview</div>
                      {/if}
                      <div>
                        <div class="name">{file.filename}</div>
                        <div class="path">{file.path}</div>
                      </div>
                    </div>
                </td>
                <td>{file.size}</td>
                <td class="hash">{file.hash.slice(0, 16)}...</td>
                <td>
                  <button type="button" class="secondary" onclick={() => loadClassification(file.id)} disabled={classBusy}>
                    Classify
                  </button>
                </td>
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
  select,
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

  select {
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

  button.danger {
    background: #9f1239;
    border-color: #9f1239;
    color: #fff;
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

  .progress-wrap {
    margin-top: 0.8rem;
    display: grid;
    gap: 0.35rem;
  }

  .subprogress-wrap {
    margin-top: 0.4rem;
    padding: 0.45rem 0.55rem;
    border: 1px solid #e2e8f0;
    border-radius: 10px;
    background: #f8fafc;
  }

  .progress-label {
    display: flex;
    justify-content: space-between;
    font-size: 0.85rem;
    color: #334155;
    gap: 0.75rem;
  }

  .progress-label.minor {
    margin-top: 0.2rem;
    color: #64748b;
    font-size: 0.78rem;
  }

  .file-name {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  progress {
    width: 100%;
    height: 0.8rem;
    border-radius: 10px;
    overflow: hidden;
    accent-color: #0c6b58;
  }

  .subprogress {
    height: 0.65rem;
    accent-color: #0f766e;
  }

  .result-grid p {
    margin: 0;
    padding: 0.5rem;
    border-radius: 9px;
    background: #eef7ff;
  }

  .checkbox-row {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    color: #334155;
    font-size: 0.9rem;
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

  .file-cell {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .thumb {
    width: 72px;
    height: 48px;
    object-fit: cover;
    border-radius: 8px;
    border: 1px solid #dbe3ef;
    background: #f8fafc;
    flex: 0 0 auto;
  }

  .thumb.placeholder {
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 0.72rem;
    color: #64748b;
  }

  .path,
  .hash,
  .muted {
    color: #64748b;
    font-size: 0.85rem;
  }

  .child-row td {
    background: #f8fafc;
  }

  .focused-container-row {
    outline: 2px solid #0f766e;
    outline-offset: -2px;
    background: #ecfdf5;
  }

  .container-actions {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
  }

  .container-thumb-grid {
    display: grid;
    grid-template-columns: repeat(4, minmax(60px, 1fr));
    gap: 0.4rem;
    margin-bottom: 0.6rem;
    max-width: 520px;
  }

  .container-thumb-grid.compact {
    grid-template-columns: repeat(4, minmax(40px, 1fr));
    gap: 0.25rem;
    margin-bottom: 0;
    max-width: 220px;
  }

  .container-preview-cell {
    min-width: 230px;
  }

  .container-thumb-cell {
    border: 1px solid #d8e0ea;
    border-radius: 8px;
    overflow: hidden;
    background: #ffffff;
  }

  .container-thumb {
    display: block;
    width: 100%;
    aspect-ratio: 4 / 3;
    object-fit: cover;
  }

  .container-thumb.placeholder {
    width: 100%;
    aspect-ratio: 4 / 3;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #64748b;
    font-size: 0.7rem;
    background: #f1f5f9;
  }

  .child-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.4rem;
  }

  .child-list li {
    display: grid;
    gap: 0.15rem;
    padding: 0.5rem 0.6rem;
    border-radius: 8px;
    background: #ffffff;
    border: 1px solid #e5e7eb;
  }

  .child-type {
    font-size: 0.75rem;
    font-weight: 700;
    color: #0f766e;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .child-name {
    font-weight: 600;
  }

  .child-path {
    color: #64748b;
    font-size: 0.8rem;
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
