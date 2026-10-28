-- saysdfTerminal: opening files in the built-in viewer (change code-viewer, group 3).
--
-- setup options (require("saysdf").setup({ viewer = {...} })); viewer = false disables the module:
--   bin = nil                 -- explicit path to the saysdf binary; otherwise next to saysdfTerm, then PATH
--   keys = true               -- Ctrl+Shift+G hotkey (path prompt); false = no hotkey
--   key = { key = "G", mods = "CTRL|SHIFT" }  -- custom hotkey
--   editor = nil              -- external editor command array for the `e` key in the viewer,
--                                e.g. { "code", "-g", "{path}:{line}:{col}" }; passed as
--                                --editor-cmd (JSON array); {path}/{line}/{col} are substituted in saysdf
--   split = { direction = "Right", size = 0.5 }  -- split parameters
--   notify = true             -- error toasts
--   markdown = { images = true }  -- images in the Markdown preview; images = false -> `saysdf md --no-images`
--   mermaid = { ascii = false, zoom = nil }  -- `saysdf mermaid --ascii --zoom compact|normal|spacious`
--
-- API:
--   open(window, pane, path, line?, col?) -> bool   -- split running `saysdf <subcommand> <path>`;
--       a relative path is resolved from the pane cwd; local domain only (otherwise a toast)
--   build_args(bin, subcommand, path, line, col, editor, extra?) -> {args}
--       extra — extra flags before `--` (for md: --no-images)
--   subcommand_flags(subcommand, opts) -> {flags}
--   subcommand_for(path) -> subcommand name by extension (table M.SUBCOMMANDS)
--   find_bin() -> path|nil (cached; opts.bin takes priority)
--   is_image(path) -> bool; image_args(cli, path) -> { cli, "imgcat", "--hold", path }
--       images (type "image" in saysdf.paths.classify) are opened not in saysdf but with `saysdfTerm-cli imgcat --hold`
--       (change inline-graphics); the binary is opts.cli_bin, otherwise next to saysdfTerm, then PATH
--
-- Binary lookup: opts.bin -> wezterm.executable_dir -> PATH (saysdf.util.find_bin). If not found —
-- a "saysdf not found" toast (not shown again until the config is reloaded).
--
-- Extension -> subcommand table: M.SUBCOMMANDS. .md/.markdown -> `md` (markdown-preview);
-- .mmd/.mermaid -> `mermaid` (mermaid-preview).

local M = {}

local t = require("saysdf.i18n").domain("viewer")

local BS = string.char(92)

local function wezterm()
  return require("wezterm")
end

M.DEFAULT_SUBCOMMAND = "view"

-- Extension (lower case, no dot) -> saysdf binary subcommand.
M.SUBCOMMANDS = {
  md = "md",
  markdown = "md",
  mmd = "mermaid",
  mermaid = "mermaid",
}

local state = { opts = {}, bin = nil, warned = false }

-- Images (change inline-graphics): type "image" per saysdf.paths.classify (a single table
-- of extensions) -> `saysdfTerm-cli imgcat --hold <path>`.
M.CLI = "saysdfTerm-cli"

function M.is_image(path)
  local ok, paths = pcall(require, "saysdf.paths")
  return ok and type(paths.classify) == "function" and paths.classify(tostring(path or "")) == "image"
end

function M.image_args(cli, path)
  return { cli, "imgcat", "--hold", path }
end

function M.subcommand_for(path)
  local ext = tostring(path or ""):match("%.([^./" .. BS .. "]+)$")
  return (ext and M.SUBCOMMANDS[ext:lower()]) or M.DEFAULT_SUBCOMMAND
end

local function json_array(list)
  local map = { ['"'] = BS .. '"', [BS] = BS .. BS }
  map[string.char(10)] = BS .. "n"
  map[string.char(13)] = BS .. "r"
  map[string.char(9)] = BS .. "t"
  local parts = {}
  for _, v in ipairs(list) do
    local s = tostring(v):gsub('[%c"' .. BS .. ']', function(c)
      return map[c] or string.format(BS .. "u%04x", c:byte())
    end)
    table.insert(parts, '"' .. s .. '"')
  end
  return "[" .. table.concat(parts, ",") .. "]"
end

function M.subcommand_flags(subcommand, opts)
  local flags = {}
  opts = opts or {}
  if subcommand == "md" and type(opts.markdown) == "table" and opts.markdown.images == false then
    table.insert(flags, "--no-images")
  end
  if subcommand == "mermaid" and type(opts.mermaid) == "table" then
    if opts.mermaid.ascii == true then
      table.insert(flags, "--ascii")
    end
    local z = opts.mermaid.zoom
    if z == "compact" or z == "normal" or z == "spacious" then
      table.insert(flags, "--zoom")
      table.insert(flags, z)
    end
  end
  return flags
end

function M.build_args(bin, subcommand, path, line, col, editor, extra)
  local args = { bin, subcommand or M.DEFAULT_SUBCOMMAND }
  if line then
    table.insert(args, "--line")
    table.insert(args, tostring(line))
  end
  if col then
    table.insert(args, "--col")
    table.insert(args, tostring(col))
  end
  if type(editor) == "table" and #editor > 0 then
    table.insert(args, "--editor-cmd")
    table.insert(args, json_array(editor))
  end
  for _, f in ipairs(extra or {}) do
    table.insert(args, f)
  end
  table.insert(args, "--")
  table.insert(args, path)
  return args
end

function M.find_bin()
  if state.bin then
    return state.bin
  end
  local bin = state.opts.bin
  if type(bin) ~= "string" or bin == "" then
    bin = require("saysdf.util").find_bin("saysdf")
  end
  state.bin = bin
  return bin
end

local function notify(window, msg)
  require("saysdf.util").log_warn("saysdf.viewer: " .. msg)
  if state.opts.notify ~= false and window then
    require("saysdf.util").notify(window, "saysdf", msg)
  end
end

local function is_absolute(path)
  return path:match("^/") or path:match("^%a:[/" .. BS .. "]") or path:match("^" .. BS .. BS)
end

local function resolve(pane, path)
  if is_absolute(path) then
    return path
  end
  local ok, paths = pcall(require, "saysdf.paths")
  if ok and type(paths.resolve) == "function" then
    local full = paths.resolve(pane, path)
    if full then
      return full
    end
  end
  return nil
end

function M.open(window, pane, path, line, col)
  if type(path) ~= "string" or path == "" then
    return false
  end
  local okd, domain = pcall(function()
    return pane:get_domain_name()
  end)
  if okd and domain ~= "local" then
    notify(window, t("the viewer works only in the local domain"))
    return false
  end
  local full = resolve(pane, path)
  if not full then
    notify(window, t("unknown pane folder: {path}", { path = path }))
    return false
  end
  local args
  if M.is_image(full) then
    local cli = state.opts.cli_bin
    if type(cli) ~= "string" or cli == "" then
      cli = require("saysdf.util").find_bin(M.CLI)
    end
    if not cli then
      notify(window, t("{name} not found (next to saysdfTerm or in PATH)", { name = M.CLI }))
      return false
    end
    args = M.image_args(cli, full)
  else
    local bin = M.find_bin()
    if not bin then
      notify(window, t("{name} not found (next to saysdfTerm or in PATH)", { name = "saysdf" }))
      return false
    end
    local sub = M.subcommand_for(full)
    args = M.build_args(bin, sub, full, line, col, state.opts.editor, M.subcommand_flags(sub, state.opts))
  end
  local sp = state.opts.split or {}
  pane:split({ args = args, direction = sp.direction or "Right", size = sp.size or 0.5 })
  return true
end

function M.prompt(window, pane)
  local w = wezterm()
  window:perform_action(
    w.action.PromptInputLine({
      description = t("Open file in the viewer (path[:line[:column]])"),
      action = w.action_callback(function(win, p, text)
        if not text or text == "" then
          return
        end
        local path, line, col = text:match("^(.-):(%d+):(%d+)$")
        if not path then
          path, line = text:match("^(.-):(%d+)$")
        end
        M.open(win, p, path or text, line and tonumber(line), col and tonumber(col))
      end),
    }),
    pane
  )
end

function M.setup(opts, config)
  opts = opts or {}
  state = { opts = opts, bin = nil }
  if opts.keys ~= false then
    local b = type(opts.key) == "table" and opts.key or { key = "G", mods = "CTRL|SHIFT" }
    local keys = config.keys or {}
    table.insert(keys, {
      key = b.key,
      mods = b.mods,
      action = wezterm().action_callback(function(window, pane)
        M.prompt(window, pane)
      end),
    })
    config.keys = keys
  end
  -- saysdfTerminal: registry entry (menu-bar 1.5): palette + "Tools" menu.
  if opts.palette ~= false then
    require("saysdf.palette").add({
      id = "viewer.open",
      menu = "Tools",
      order = 10,
      label = t("Open file…"),
      accel = "o",
      key = opts.keys ~= false
          and require("saysdf.palette").key_text(type(opts.key) == "table" and opts.key or { key = "G", mods = "CTRL|SHIFT" })
        or nil,
      hint = { contexts = { "shell", "git" }, label = t("File"), priority = 50 },
      brief = t("Viewer: open file…"),
      icon = "md_file_eye",
      action = wezterm().action_callback(function(window, pane)
        M.prompt(window, pane)
      end),
    })
  end
  return config
end

M._state = function()
  return state
end

return M
