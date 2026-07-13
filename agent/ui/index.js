document.addEventListener('DOMContentLoaded', () => {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;

  // DOM Elements
  const agentStatusBadge = document.getElementById('agent-status-badge');
  const agentStatusText = document.getElementById('agent-status-text');

  const agentIdVal = document.getElementById('agent-id-val');
  
  const serverUrlInput = document.getElementById('server-url');
  const serverTokenInput = document.getElementById('server-token');
  const agentNameInput = document.getElementById('agent-name');

  const autostartAgentInput = document.getElementById('autostart-agent');
  const closeToTrayAgentInput = document.getElementById('close-to-tray-agent');
  
  const updateBanner = document.getElementById('update-banner');
  const updateText = document.getElementById('update-text');
  const updateBtn = document.getElementById('update-btn');

  const toggleAgentBtn = document.getElementById('toggle-agent-btn');
  const saveConfigBtn = document.getElementById('save-config-btn');
  const importConfigBtn = document.getElementById('import-config-btn');
  const clearLogsBtn = document.getElementById('clear-logs-btn');
  const copyLogsBtn = document.getElementById('copy-logs-btn');
  const consoleOutput = document.getElementById('console-output');

  let isAgentRunning = false;

  // Append a message to the console output.
  // Keeps only the last MAX_LOG_LINES to prevent unbounded DOM growth (and the
  // resulting WebView2 layout-engine CPU drain) during multi-hour agent sessions.
  const MAX_LOG_LINES = 500;
  function appendLog(message) {
    if (!message) return;

    const line = document.createElement('div');
    line.className = 'log-line';

    // Parse level to add styling class
    if (message.includes('INFO') || message.includes('info')) {
      line.classList.add('log-info');
    } else if (message.includes('WARN') || message.includes('warn')) {
      line.classList.add('log-warn');
    } else if (message.includes('ERROR') || message.includes('error') || message.includes('fail')) {
      line.classList.add('log-error');
    } else if (message.includes('DEBUG') || message.includes('debug')) {
      line.classList.add('log-debug');
    }

    line.textContent = message;
    consoleOutput.appendChild(line);

    // Trim oldest lines when over the limit
    while (consoleOutput.children.length > MAX_LOG_LINES) {
      consoleOutput.removeChild(consoleOutput.firstChild);
    }

    // Auto-scroll to bottom if scrolled near bottom
    const threshold = 40;
    const isNearBottom = consoleOutput.scrollHeight - consoleOutput.clientHeight - consoleOutput.scrollTop < threshold;
    if (isNearBottom || consoleOutput.scrollTop === 0) {
      consoleOutput.scrollTop = consoleOutput.scrollHeight;
    }
  }

  // Load configuration from Rust backend
  async function loadConfig() {
    try {
      const config = await invoke('get_config');
      serverUrlInput.value = config.server_url || 'ws://127.0.0.1:8080';
      serverTokenInput.value = config.server_token || '';
      agentNameInput.value = config.agent_name || '';

      autostartAgentInput.checked = !!config.autostart;
      closeToTrayAgentInput.checked = !!config.close_to_tray;
      agentIdVal.textContent = config.client_unique_id || 'N/A';
      
      appendLog(`[INFO] Loaded configuration. Agent ID: ${config.client_unique_id}`);
    } catch (err) {
      appendLog(`[ERROR] Failed to load configuration: ${err}`);
    }
  }

  // Save configuration to Rust backend
  async function saveConfig() {
    try {
      const serverUrl = serverUrlInput.value.trim();
      const serverToken = serverTokenInput.value.trim();
      const agentName = agentNameInput.value.trim();

      const autostart = autostartAgentInput.checked;
      const closeToTray = closeToTrayAgentInput.checked;

      await invoke('save_config', {
        serverUrl,
        agentName,
        serverToken,
        autostart,
        closeToTray
      });

      appendLog(`[INFO] Configuration saved successfully.`);
      
      // Flash save button green momentarily
      saveConfigBtn.style.background = 'var(--success-color)';
      setTimeout(() => {
        saveConfigBtn.style.background = '';
      }, 1000);
    } catch (err) {
      appendLog(`[ERROR] Failed to save configuration: ${err}`);
    }
  }

  // Start or Stop the Agent loop
  async function toggleAgent() {
    toggleAgentBtn.disabled = true;
    try {
      if (isAgentRunning) {
        appendLog(`[INFO] Stopping Host Agent...`);
        await invoke('stop_agent');
      } else {
        appendLog(`[INFO] Starting Host Agent...`);
        // Save config first to make sure current fields are used
        await saveConfig();
        await invoke('start_agent');
      }
    } catch (err) {
      appendLog(`[ERROR] Action failed: ${err}`);
      toggleAgentBtn.disabled = false;
    }
  }

  // Check state and update UI
  async function pollStatus() {
    try {
      const status = await invoke('get_status');
      
      // Update Agent State
      isAgentRunning = status.agent_active;
      if (isAgentRunning) {
        agentStatusBadge.className = 'status-badge status-online';
        agentStatusText.textContent = status.connected_to_server ? 'Connected' : 'Connecting';
        toggleAgentBtn.textContent = 'Stop Host Agent';
        toggleAgentBtn.className = 'btn btn-danger';
      } else {
        agentStatusBadge.className = 'status-badge status-offline';
        agentStatusText.textContent = 'Inactive';
        toggleAgentBtn.textContent = 'Start Host Agent';
        toggleAgentBtn.className = 'btn btn-primary';
      }
      toggleAgentBtn.disabled = false;



      // If an error is reported from the background loop
      if (status.last_error) {
        appendLog(`[ERROR] Agent Background Error: ${status.last_error}`);
        // Clear error in backend to prevent repeating logs
        await invoke('clear_last_error');
      }
    } catch (err) {
      console.error('Error polling status:', err);
    }
  }

  // Event Listeners
  toggleAgentBtn.addEventListener('click', toggleAgent);
  saveConfigBtn.addEventListener('click', saveConfig);
  importConfigBtn.addEventListener('click', async () => {
    try {
      appendLog(`[INFO] Requesting configuration file import...`);
      const success = await invoke('import_config');
      if (success) {
        appendLog(`[INFO] Configuration imported successfully!`);
        await loadConfig();
      } else {
        appendLog(`[INFO] Configuration import cancelled.`);
      }
    } catch (err) {
      appendLog(`[ERROR] Failed to import configuration: ${err}`);
    }
  });
  
  clearLogsBtn.addEventListener('click', () => {
    consoleOutput.innerHTML = '';
  });

  copyLogsBtn.addEventListener('click', () => {
    const text = consoleOutput.innerText;
    navigator.clipboard.writeText(text).then(() => {
      appendLog(`[INFO] Logs copied to clipboard.`);
    }).catch(err => {
      appendLog(`[ERROR] Failed to copy logs: ${err}`);
    });
  });

  // Listen to logs from Rust
  listen('log-message', (event) => {
    appendLog(event.payload);
  });

  // Init
  loadConfig();
  pollStatus();
  
  // Poll status every 2 s (was 1 s). The status changes infrequently and each
  // poll crosses the JS↔Rust IPC bridge; halving the rate saves CPU without
  // noticeable UX impact.
  setInterval(pollStatus, 2000);

  // Check for updates
  async function checkForUpdates() {
    try {
      const update = await invoke('check_for_updates');
      if (update) {
        updateText.textContent = `A new version (${update.latest_version}) of Lunaris Agent is available!`;
        updateBanner.style.display = 'flex';
        updateBtn.addEventListener('click', () => {
          invoke('open_url', { url: update.release_url });
        });
      }
    } catch (err) {
      console.error('Failed to check for updates:', err);
    }
  }
  
  checkForUpdates();
});
