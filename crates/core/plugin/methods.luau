--[[
	The Studio side of every RoVibe tool. The app sends this chunk to the
	plugin each time it connects, so changing a tool only takes restarting the
	app: Studio, and whatever is unsaved in it, stays open.

	`env` is what the plugin lends this code: `getSyncController()`.
]]
return function(env)
	local ChangeHistoryService = game:GetService("ChangeHistoryService")
	local CollectionService = game:GetService("CollectionService")
	local EncodingService = game:GetService("EncodingService")
	local RunService = game:GetService("RunService")
	local SerializationService = game:GetService("SerializationService")

	local ROOT_SERVICES = {
		"Workspace",
		"Players",
		"Lighting",
		"MaterialService",
		"ReplicatedFirst",
		"ReplicatedStorage",
		"ServerScriptService",
		"ServerStorage",
		"StarterGui",
		"StarterPack",
		"StarterPlayer",
		"Teams",
		"SoundService",
		"TextChatService",
	}

	local COMMON_PROPERTIES = {
		"Anchored",
		"CanCollide",
		"CFrame",
		"Color",
		"Enabled",
		"Material",
		"Position",
		"Size",
		"Text",
		"Transparency",
		"Value",
		"Visible",
	}


	local function namedChildren(parent, name)
		local found = {}
		for _, child in parent:GetChildren() do
			if child.Name == name then
				table.insert(found, child)
			end
		end
		return found
	end

	-- A path that names one instance only: `Name[n]` marks the n-th of
	-- several siblings sharing a name, in the order Studio lists them.
	local function getFullPath(instance)
		if instance == game then
			return "game"
		end

		local names = {}
		local current = instance
		while current and current ~= game do
			local segment = current.Name
			local parent = current.Parent
			if parent then
				local twins = namedChildren(parent, current.Name)
				if #twins > 1 then
					segment ..= "[" .. tostring(table.find(twins, current)) .. "]"
				end
			end
			table.insert(names, 1, segment)
			current = parent
		end
		return "game." .. table.concat(names, ".")
	end

	local function resolvePath(path)
		if path == nil or path == "" or path == "game" then
			return game
		end

		local separator = if string.find(path, "/", 1, true) then "/" else "."
		local segments = string.split(path, separator)
		if segments[1] == "game" then
			table.remove(segments, 1)
		end

		local current = game
		for index, segment in segments do
			local name, nth = segment, nil
			local base, number = string.match(segment, "^(.-)%[(%d+)%]$")
			if base and current:FindFirstChild(segment) == nil then
				name, nth = base, tonumber(number)
			end

			local twins = namedChildren(current, name)
			local child
			if nth then
				child = twins[nth]
				if child == nil then
					return nil, string.format("%s n'a que %d enfant(s) nommé(s) '%s'", getFullPath(current), #twins, name)
				end
			elseif #twins > 1 then
				-- Picking one silently is how a tool reads one tree and a test
				-- walks to another: the order differs from one DataModel to
				-- the next.
				return nil, string.format(
					"chemin ambigu : %d enfants de %s s'appellent '%s'. Désigne-en un avec %s[n] (voir `search`), ou renomme-les.",
					#twins,
					getFullPath(current),
					name,
					name
				)
			else
				child = twins[1]
			end
			if child == nil and index == 1 then
				local ok, service = pcall(game.GetService, game, name)
				child = if ok then service else nil
			end
			if child == nil then
				return nil, string.format("'%s' not found under %s", name, getFullPath(current))
			end
			current = child
		end
		return current
	end

	local function describe(value, depth)
		local valueType = typeof(value)
		if valueType == "string" or valueType == "number" or valueType == "boolean" or valueType == "nil" then
			return value
		elseif valueType == "Instance" then
			return string.format("%s [%s]", getFullPath(value), value.ClassName)
		elseif valueType == "table" then
			if depth <= 0 then
				return "{...}"
			end
			local result = {}
			local count = 0
			for key, item in value do
				count += 1
				if count > 200 then
					result["..."] = "truncated"
					break
				end
				result[tostring(key)] = describe(item, depth - 1)
			end
			return result
		end
		return string.format("%s(%s)", valueType, tostring(value))
	end

	local function formatArgs(...)
		local parts = table.pack(...)
		for index = 1, parts.n do
			parts[index] = tostring(parts[index])
		end
		return table.concat(parts, " ", 1, parts.n)
	end

	local methods = {}

	function methods.run_luau(params)
		-- print/warn are captured so the caller gets its own output back instead
		-- of having to fish it out of the shared console log.
		local output = {}
		local function capturedPrint(...)
			table.insert(output, formatArgs(...))
		end
		local function capturedWarn(...)
			table.insert(output, "[warn] " .. formatArgs(...))
		end

		local fn
		local available, loaded, compileError = pcall(loadstring, params.code, "=rovibe")
		if available then
			if not loaded then
				error("Compile error: " .. tostring(compileError), 0)
			end
			setfenv(loaded, setmetatable({ print = capturedPrint, warn = capturedWarn }, { __index = getfenv(loaded) }))
			fn = loaded
		else
			-- loadstring is off in a running game unless the place opts in, and it
			-- never exists on the client. A ModuleScript built on the fly runs the
			-- same code everywhere; its parameters shadow the global print/warn.
			local module = Instance.new("ModuleScript")
			module.Name = "rovibe"
			module.Source = "return function(print, warn)\n" .. params.code .. "\nend"
			local ok, body = pcall(require, module)
			module:Destroy()
			if not ok then
				error("Compile error (détail dans get_console) : " .. tostring(body), 0)
			end
			fn = function()
				return body(capturedPrint, capturedWarn)
			end
		end

		local recording = nil
		if RunService:IsEdit() then
			local ok, id = pcall(ChangeHistoryService.TryBeginRecording, ChangeHistoryService, "RoVibe: run_luau")
			recording = if ok then id else nil
		end

		local finished, results = false, nil
		local thread = task.spawn(function()
			results = table.pack(xpcall(fn, function(message)
				-- Frames from this module are noise to whoever wrote the snippet.
				local lines = {}
				for _, line in string.split(debug.traceback(tostring(message), 2), "\n") do
					if line ~= "" and not string.find(line, "RoVibeMethods", 1, true) then
						table.insert(lines, line)
					end
				end
				return table.concat(lines, "\n")
			end))
			finished = true
		end)

		local deadline = os.clock() + (params.timeout or 30)
		while not finished and os.clock() < deadline do
			task.wait()
		end

		if recording then
			pcall(ChangeHistoryService.FinishRecording, ChangeHistoryService, recording, Enum.FinishRecordingOperation.Commit)
		end

		if not finished then
			task.cancel(thread)
			return { ok = false, output = table.concat(output, "\n"), error = "Timed out" }
		end

		if not results[1] then
			return { ok = false, output = table.concat(output, "\n"), error = results[2] }
		end

		local returns = {}
		for index = 2, results.n do
			table.insert(returns, describe(results[index], 4))
		end
		return { ok = true, output = table.concat(output, "\n"), returns = returns }
	end

	function methods.get_tree(params)
		local root, resolveError = resolvePath(params.path)
		if not root then
			error(resolveError, 0)
		end

		local maxDepth = params.depth or 2
		local budget = params.max or 300
		local lines = {}

		local function visit(instance, depth)
			if budget <= 0 then
				return
			end
			budget -= 1

			local children = instance:GetChildren()
			local suffix = if #children > 0 and depth >= maxDepth then string.format(" (%d children)", #children) else ""
			table.insert(lines, string.rep("  ", depth) .. instance.Name .. " [" .. instance.ClassName .. "]" .. suffix)

			if depth < maxDepth then
				for _, child in children do
					visit(child, depth + 1)
				end
			end
		end

		if root == game then
			for _, serviceName in ROOT_SERVICES do
				local service = game:FindService(serviceName)
				if service then
					visit(service, 0)
				end
			end
		else
			visit(root, 0)
		end

		if budget <= 0 then
			table.insert(lines, "... truncated, narrow the path or lower the depth")
		end
		return table.concat(lines, "\n")
	end

	function methods.search(params)
		local root, resolveError = resolvePath(params.root)
		if not root then
			error(resolveError, 0)
		end

		local query = if params.query then string.lower(params.query) else nil
		local limit = params.limit or 50
		local found = {}

		local function scan(container)
			for _, instance in container:GetDescendants() do
				if #found >= limit then
					return
				end
				local matchesClass = params.class == nil or instance:IsA(params.class)
				local matchesName = query == nil or string.find(string.lower(instance.Name), query, 1, true) ~= nil
				if matchesClass and matchesName then
					table.insert(found, getFullPath(instance) .. " [" .. instance.ClassName .. "]")
				end
			end
		end

		if root == game then
			for _, serviceName in ROOT_SERVICES do
				local service = game:FindService(serviceName)
				if service then
					scan(service)
				end
			end
		else
			scan(root)
		end

		return table.concat(found, "\n")
	end

	function methods.get_instance(params)
		local instance, resolveError = resolvePath(params.path)
		if not instance then
			error(resolveError, 0)
		end

		local properties = {}
		for _, name in COMMON_PROPERTIES do
			local ok, value = pcall(function()
				return (instance :: any)[name]
			end)
			if ok and typeof(value) ~= "Instance" then
				properties[name] = describe(value, 1)
			end
		end

		local children = {}
		for _, child in instance:GetChildren() do
			if #children >= 200 then
				table.insert(children, "...")
				break
			end
			table.insert(children, child.Name .. " [" .. child.ClassName .. "]")
		end

		local result = {
			path = getFullPath(instance),
			class = instance.ClassName,
			properties = properties,
			attributes = describe(instance:GetAttributes(), 2),
			tags = CollectionService:GetTags(instance),
			children = children,
		}

		if params.source and instance:IsA("LuaSourceContainer") then
			result.source = (instance :: any).Source
		end
		return result
	end

	function methods.playtest(params)
		local StudioTestService = game:GetService("StudioTestService")

		if params.action == "stop" then
			-- The argument becomes the return value of Execute*ModeAsync; nil is rejected.
			StudioTestService:EndTest("stopped")
			return "stopped"
		end

		if not RunService:IsEdit() then
			error("A playtest is already running", 0)
		end

		local players = math.clamp(math.floor(tonumber(params.players) or 1), 1, 8)
		-- Execute*Async only returns when the test ends, so it runs detached.
		task.spawn(function()
			if params.mode == "run" then
				StudioTestService:ExecuteRunModeAsync({})
			elseif players > 1 then
				StudioTestService:ExecuteMultiplayerTestAsync(players, {})
			else
				StudioTestService:ExecutePlayModeAsync({})
			end
		end)
		return "started"
	end

	-- How the game runs, measured over a few seconds: frame rate and frame
	-- times from Heartbeat, the engine's own step times, memory and what the
	-- scene holds. In the client during a test, it is what a player feels.
	function methods.performance_check(params)
		local Stats = game:GetService("Stats")
		local duration = math.clamp(tonumber(params.duration) or 3, 0.5, 8)
		local read = function(name)
			local ok, value = pcall(function()
				return (Stats :: any)[name]
			end)
			return if ok and type(value) == "number" then value else nil
		end

		local frames = {}
		local connection = RunService.Heartbeat:Connect(function(step)
			table.insert(frames, step)
		end)
		local memoryStart = Stats:GetTotalMemoryUsageMb()
		local memoryPeak = memoryStart
		local heartbeat, physics, samples = 0, 0, 0
		local started = os.clock()
		while os.clock() - started < duration do
			task.wait(0.25)
			samples += 1
			heartbeat += read("HeartbeatTime") or 0
			physics += read("PhysicsStepTime") or 0
			memoryPeak = math.max(memoryPeak, Stats:GetTotalMemoryUsageMb())
		end
		connection:Disconnect()
		local elapsed = os.clock() - started
		local memoryEnd = Stats:GetTotalMemoryUsageMb()

		local context = if RunService:IsEdit() then "edit" elseif RunService:IsServer() then "server" else "client"
		local lines = { string.format("Mesuré %.1f s dans le DataModel %s.", elapsed, context) }
		if #frames == 0 then
			table.insert(lines, "Aucune image : le jeu ne tourne pas (Studio réduit ou en pause ?).")
		else
			local sorted = table.clone(frames)
			table.sort(sorted)
			local total = 0
			for _, step in frames do
				total += step
			end
			local at = function(share)
				return sorted[math.clamp(math.ceil(#sorted * share), 1, #sorted)] * 1000
			end
			table.insert(lines, string.format(
				"Images : %d, soit %.1f par seconde. Temps d'image : moyen %.1f ms, médian %.1f ms, p95 %.1f ms, pire %.1f ms.",
				#frames, #frames / elapsed, total / #frames * 1000, at(0.5), at(0.95), sorted[#sorted] * 1000
			))
			local slow = 0
			for _, step in frames do
				if step > 1 / 30 then
					slow += 1
				end
			end
			if slow > 0 then
				table.insert(lines, string.format("%d image(s) au-dessus de 33 ms (sous 30 images par seconde).", slow))
			end
		end
		-- The editor reports no step times: a line of zeros would mislead.
		if samples > 0 and heartbeat + physics > 0 then
			table.insert(lines, string.format(
				"Moteur : Heartbeat %.2f ms, physique %.2f ms en moyenne.",
				heartbeat / samples * 1000, physics / samples * 1000
			))
		end
		table.insert(lines, string.format(
			"Mémoire : %.0f Mo au début, %.0f Mo à la fin, %.0f Mo au plus haut.",
			memoryStart, memoryEnd, memoryPeak
		))
		local scene = {}
		for _, item in {
			{ "InstanceCount", "instances" },
			{ "PrimitivesCount", "pièces physiques" },
			{ "MovingPrimitivesCount", "en mouvement" },
			{ "ContactsCount", "contacts" },
			{ "SceneDrawcallCount", "appels de dessin" },
			{ "SceneTriangleCount", "triangles affichés" },
		} do
			local value = read(item[1])
			if value and value > 0 then
				table.insert(scene, string.format("%d %s", value, item[2]))
			end
		end
		if #scene > 0 then
			table.insert(lines, "Scène : " .. table.concat(scene, ", ") .. ".")
		end
		return table.concat(lines, "\n")
	end

	local function collectInstances(roots)
		local all = {}
		for _, root in roots do
			table.insert(all, root)
			for _, descendant in root:GetDescendants() do
				table.insert(all, descendant)
			end
		end
		return all
	end

	function methods.asset_export(params)
		local instance, resolveError = resolvePath(params.path)
		if not instance then
			error(resolveError, 0)
		end
		if instance == game or instance.Parent == game then
			error("Enregistre une instance précise, pas un service entier", 0)
		end

		local data = SerializationService:SerializeInstancesAsync({ instance })
		return {
			data = buffer.tostring(EncodingService:Base64Encode(data)),
			name = instance.Name,
			class = instance.ClassName,
			count = #instance:GetDescendants() + 1,
		}
	end

	function methods.asset_insert(params)
		local parent, resolveError = resolvePath(params.parent or "game.Workspace")
		if not parent then
			error(resolveError, 0)
		end

		local roots
		if params.data then
			local data = EncodingService:Base64Decode(buffer.fromstring(params.data))
			roots = SerializationService:DeserializeInstancesAsync(data)
		elseif params.kind == "audio" then
			-- Audio assets aren't instances; a Sound pointing at the id is what
			-- the Toolbox itself inserts.
			local sound = Instance.new("Sound")
			sound.SoundId = "rbxassetid://" .. tostring(params.assetId)
			roots = { sound }
		else
			roots = game:GetObjects("rbxassetid://" .. tostring(params.assetId))
		end

		if #roots == 0 then
			error("L'asset ne contient aucune instance", 0)
		end

		local disabledScripts, moduleScripts = 0, 0
		for _, instance in collectInstances(roots) do
			if instance:IsA("BaseScript") then
				if params.disableScripts then
					(instance :: any).Enabled = false
					disabledScripts += 1
				end
			elseif instance:IsA("ModuleScript") then
				moduleScripts += 1
			end
		end

		local recording = nil
		if RunService:IsEdit() then
			local ok, id = pcall(ChangeHistoryService.TryBeginRecording, ChangeHistoryService, "RoVibe: asset_insert")
			recording = if ok then id else nil
		end

		local inserted = {}
		for _, root in roots do
			if params.name and #roots == 1 then
				root.Name = params.name
			end
			if params.position and root:IsA("PVInstance") then
				root:PivotTo(CFrame.new(params.position[1], params.position[2], params.position[3]))
			end
			root.Parent = parent

			local line = getFullPath(root) .. " [" .. root.ClassName .. "]"
			if root:IsA("Model") then
				local size = root:GetExtentsSize()
				line ..= string.format(" taille %.1f x %.1f x %.1f", size.X, size.Y, size.Z)
			elseif root:IsA("BasePart") then
				line ..= string.format(" taille %.1f x %.1f x %.1f", root.Size.X, root.Size.Y, root.Size.Z)
			end
			table.insert(inserted, line)
		end

		if recording then
			pcall(ChangeHistoryService.FinishRecording, ChangeHistoryService, recording, Enum.FinishRecordingOperation.Commit)
		end

		local lines = { "Inséré :", table.concat(inserted, "\n") }
		if disabledScripts > 0 then
			table.insert(
				lines,
				string.format("%d script(s) désactivé(s) par précaution : lis-les avant de les réactiver.", disabledScripts)
			)
		end
		if moduleScripts > 0 then
			table.insert(
				lines,
				string.format("%d ModuleScript(s) présent(s) : ils s'exécutent dès qu'un script les require.", moduleScripts)
			)
		end
		return table.concat(lines, "\n")
	end

	-- Takes a picture of this DataModel's view with Roblox's own capture.
	-- Returns a temporary content id, which only the edit DataModel is
	-- allowed to turn into pixels (capture_read), even for a test client.
	function methods.capture_start()
		local id = nil
		game:GetService("CaptureService"):CaptureScreenshot(function(contentId)
			id = contentId
		end)
		local deadline = os.clock() + 5
		while not id and os.clock() < deadline do
			task.wait()
		end
		if not id then
			error("Studio n'a rendu aucune capture en 5 s (vue 3D réduite ou masquée ?)", 0)
		end
		return id
	end

	-- The pixels of a capture, RGBA, zstd-compressed then base64-encoded:
	-- a viewport is several megabytes raw and well under half that packed.
	function methods.capture_read(params)
		local image = game:GetService("AssetService"):CreateEditableImageAsync(Content.fromUri(params.id))
		local size = image.Size
		local pixels = image:ReadPixelsBuffer(Vector2.zero, size)
		image:Destroy()
		local packed = EncodingService:CompressBuffer(pixels, Enum.CompressionAlgorithm.Zstd, 3)
		return {
			width = size.X,
			height = size.Y,
			data = buffer.tostring(EncodingService:Base64Encode(packed)),
		}
	end

	-- Key names agents use, mapped to Enum.KeyCode; any KeyCode name works too.
	local KEY_ALIASES = {
		Enter = "Return",
		Ctrl = "LeftControl",
		Control = "LeftControl",
		Shift = "LeftShift",
		Alt = "LeftAlt",
		Esc = "Escape",
		Backspace = "Backspace",
		["0"] = "Zero", ["1"] = "One", ["2"] = "Two", ["3"] = "Three", ["4"] = "Four",
		["5"] = "Five", ["6"] = "Six", ["7"] = "Seven", ["8"] = "Eight", ["9"] = "Nine",
	}

	local function keyCode(name)
		local wanted = KEY_ALIASES[name] or (if #name == 1 then string.upper(name) else name)
		local ok, code = pcall(function()
			return (Enum.KeyCode :: any)[wanted]
		end)
		if not ok or code == nil then
			error(string.format("Touche inconnue : %s (lettres, chiffres, Space, Enter, Shift, Ctrl, F1…, ou un nom de Enum.KeyCode)", tostring(name)), 0)
		end
		return code
	end

	local BUTTONS = {
		left = Enum.UserInputType.MouseButton1,
		right = Enum.UserInputType.MouseButton2,
		middle = Enum.UserInputType.MouseButton3,
	}

	-- Whether VirtualInput exists here: Roblox added it in 2026.
	function methods.vinput_available()
		local ok, created = pcall(function()
			return (game:GetService("UserInputService") :: any):CreateVirtualInput()
		end)
		return ok and created ~= nil
	end

	-- Plays input steps into the game through Roblox's VirtualInput: real
	-- engine input, so the game sees it exactly as a player's, with neither
	-- the window's focus nor the user's keyboard and mouse involved. Clicks
	-- on an interface element report whether it really activated.
	function methods.vinput(params)
		local UserInputService = game:GetService("UserInputService")
		local virtual = (UserInputService :: any):CreateVirtualInput()
		local camera = workspace.CurrentCamera
		local held = {}
		local lines = {}

		local function release()
			for code in held do
				pcall(virtual.SendKey, virtual, false, code, false)
			end
			table.clear(held)
		end

		local function point(value, what)
			if type(value) ~= "table" or type(value[1]) ~= "number" or type(value[2]) ~= "number" then
				error(what .. " attend [x, y]", 0)
			end
			return Vector2.new(value[1], value[2])
		end

		local function click(position, button)
			virtual:SendMousePosition(position)
			task.wait(0.03)
			virtual:SendMouseButton(position, button, true, 1)
			task.wait(0.05)
			virtual:SendMouseButton(position, button, false, 1)
		end

		-- Moves the pointer from one point to another over `ms`, as a hand
		-- would, so that drags register as drags.
		local function glide(from, to, ms)
			local steps = math.max(2, math.floor(ms / 16))
			for index = 1, steps do
				virtual:SendMousePosition(from:Lerp(to, index / steps))
				task.wait(ms / 1000 / steps)
			end
		end

		local ok, failure = pcall(function()
			for index, step in params.steps do
				local button = BUTTONS[step.button or "left"]
				if not button then
					error("`button` vaut left, right ou middle", 0)
				end
				local ms = math.clamp(step.duration or 400, 50, 5000)

				if step.key ~= nil then
					local names = if type(step.key) == "table" then step.key else { step.key }
					local codes = {}
					for _, name in names do
						local code = keyCode(name)
						table.insert(codes, code)
						virtual:SendKey(true, code, false)
						held[code] = true
					end
					task.wait(math.clamp(step.hold or 80, 0, 5000) / 1000)
					for position = #codes, 1, -1 do
						virtual:SendKey(false, codes[position], false)
						held[codes[position]] = nil
					end
				elseif step.gui ~= nil then
					local center = methods.gui_center({ path = step.gui, locked = true })
					local target = resolvePath((string.gsub(step.gui, "Players%.LocalPlayer", "Players." .. game:GetService("Players").LocalPlayer.Name, 1)))
					local activated = false
					local connection = if target and target:IsA("GuiButton")
						then (target :: any).Activated:Connect(function()
							activated = true
						end)
						else nil
					click(Vector2.new(center.x, center.y), button)
					task.wait(0.15)
					if target and target:IsA("TextBox") then
						-- Focus lands a few frames after the click.
						local deadline = os.clock() + 0.6
						while not (target :: any):IsFocused() and os.clock() < deadline do
							task.wait()
						end
						table.insert(lines, string.format("étape %d : %s %s", index, step.gui, if (target :: any):IsFocused() then "a le focus" else "n'a PAS pris le focus"))
					end
					if connection then
						connection:Disconnect()
						table.insert(lines, string.format("étape %d : %s %s", index, step.gui, if activated then "activé" else "PAS activé (recouvert, désactivé, ou le jeu bloque la souris)"))
						if not activated and step.verify ~= false then
							error(string.format("le clic sur %s ne l'a pas activé", step.gui), 0)
						end
					end
				elseif step.click ~= nil then
					click(point(step.click, "`click`"), button)
				elseif step.move ~= nil then
					virtual:SendMousePosition(point(step.move, "`move`"))
				elseif step.drag ~= nil then
					local from, to = point(step.drag.from, "`drag.from`"), point(step.drag.to, "`drag.to`")
					virtual:SendMousePosition(from)
					virtual:SendMouseButton(from, button, true, 1)
					glide(from, to, ms)
					virtual:SendMouseButton(to, button, false, 1)
				elseif step.look ~= nil then
					local delta = point(step.look, "`look`")
					-- A locked mouse turns the camera by its movement alone.
					-- MouseBehavior can read LockCenter while a Modal button
					-- has freed the cursor: VirtualInput then refuses deltas.
					local locked = UserInputService.MouseBehavior == Enum.MouseBehavior.LockCenter
						and pcall(virtual.SendMouseDelta, virtual, Vector2.zero)
					if locked then
						local steps = math.max(2, math.floor(ms / 16))
						for _ = 1, steps do
							virtual:SendMouseDelta(delta / steps)
							task.wait(ms / 1000 / steps)
						end
					else
						-- The default camera turns while the right button is held.
						local center = camera.ViewportSize / 2
						virtual:SendMousePosition(center)
						virtual:SendMouseButton(center, Enum.UserInputType.MouseButton2, true, 1)
						glide(center, center + delta, ms)
						virtual:SendMouseButton(center + delta, Enum.UserInputType.MouseButton2, false, 1)
					end
				elseif step.scroll ~= nil then
					local at = if step.at then point(step.at, "`at`") else camera.ViewportSize / 2
					virtual:SendMousePosition(at)
					virtual:SendPointerAction(at, { Wheel = math.clamp(step.scroll, -40, 40) })
				elseif step.text ~= nil then
					local deadline = os.clock() + 1
					while not UserInputService:GetFocusedTextBox() and os.clock() < deadline do
						task.wait()
					end
					local focused = UserInputService:GetFocusedTextBox()
					if not focused then
						error(string.format("étape %d : aucune zone de saisie n'a le focus ; clique-la d'abord avec `gui`", index), 0)
					end
					virtual:SendTextInput(tostring(step.text))
					task.wait(0.05)
					table.insert(lines, string.format("étape %d : %s contient « %s »", index, getFullPath(focused), focused.Text))
				elseif step.wait ~= nil then
					task.wait(math.clamp(step.wait, 0, 5000) / 1000)
				else
					error(string.format("étape %d : il faut `key`, `gui`, `click`, `move`, `drag`, `look`, `scroll`, `text` ou `wait`", index), 0)
				end
			end
		end)
		-- Whatever happened, no key stays down in the game.
		release()
		if not ok then
			error(tostring(failure), 0)
		end
		return table.concat(lines, "\n")
	end

	function methods.viewport_info()
		local size = workspace.CurrentCamera.ViewportSize
		return { width = math.round(size.X), height = math.round(size.Y) }
	end

	function methods.gui_center(params)
		local path = params.path
		-- Agents naturally write the path the way a LocalScript would.
		local localPlayer = game:GetService("Players").LocalPlayer
		if localPlayer then
			path = string.gsub(path, "Players%.LocalPlayer", "Players." .. localPlayer.Name, 1)
		end

		local instance, resolveError = resolvePath(path)
		if not instance then
			error(resolveError, 0)
		end
		if not instance:IsA("GuiObject") then
			error(getFullPath(instance) .. " n'est pas un GuiObject", 0)
		end

		local current = instance
		while current and current:IsA("GuiObject") do
			if not current.Visible then
				error(getFullPath(current) .. " n'est pas visible", 0)
			end
			current = current.Parent
		end
		if current and current:IsA("ScreenGui") and not current.Enabled then
			error(getFullPath(current) .. " est désactivé", 0)
		end

		-- With the mouse locked, as in first person, the game reports every
		-- click at the centre of the screen wherever the cursor really is.
		-- VirtualInput clicks say afterwards whether the button activated,
		-- which settles it better than refusing up front.
		if not params.locked and game:GetService("UserInputService").MouseBehavior ~= Enum.MouseBehavior.Default then
			error("Le jeu verrouille la souris (vue à la première personne ou visée) : un clic ne peut pas viser cet élément", 0)
		end

		-- AbsolutePosition is measured from below the top bar, the viewport from
		-- its very top.
		local inset = game:GetService("GuiService"):GetGuiInset()
		local center = instance.AbsolutePosition + instance.AbsoluteSize / 2 + inset
		return { x = math.round(center.X), y = math.round(center.Y) }
	end

	function methods.play_move(params)
		local player = game:GetService("Players").LocalPlayer
		if not player then
			error("play_move ne fonctionne que dans le client d'un test Play", 0)
		end

		local character = player.Character or player.CharacterAdded:Wait()
		local humanoid = character:WaitForChild("Humanoid", 10)
		local root = character:WaitForChild("HumanoidRootPart", 10)
		if not humanoid or not root then
			error("Le personnage n'est pas prêt", 0)
		end

		local target
		if type(params.to) == "table" then
			target = Vector3.new(params.to[1], params.to[2], params.to[3])
		else
			local instance, resolveError = resolvePath(params.to)
			if not instance then
				error(resolveError, 0)
			end
			if not instance:IsA("PVInstance") then
				error(getFullPath(instance) .. " n'a pas de position", 0)
			end
			target = instance:GetPivot().Position
		end

		local deadline = os.clock() + (params.timeout or 20)
		local path = game:GetService("PathfindingService"):CreatePath({ AgentCanJump = true })
		local computed = pcall(path.ComputeAsync, path, root.Position, target)
		local hasPath = computed and path.Status == Enum.PathStatus.Success

		-- Without a navigable path, walking straight at the target still gets a
		-- useful result on open ground, and says so.
		local waypoints = if hasPath then path:GetWaypoints() else { { Position = target } }
		local flat = Vector3.new(1, 0, 1)

		for _, waypoint in waypoints do
			if waypoint.Action == Enum.PathWaypointAction.Jump then
				humanoid:ChangeState(Enum.HumanoidStateType.Jumping)
			end
			humanoid:MoveTo(waypoint.Position)

			local reissue = os.clock() + 1
			while ((root.Position - waypoint.Position) * flat).Magnitude > 2.5 do
				if os.clock() >= deadline or humanoid.Health <= 0 then
					break
				end
				-- MoveTo gives up on its own after 8 seconds.
				if os.clock() >= reissue then
					humanoid:MoveTo(waypoint.Position)
					reissue = os.clock() + 1
				end
				task.wait(0.05)
			end

			if os.clock() >= deadline or humanoid.Health <= 0 then
				break
			end
		end

		local position = root.Position
		local notes = {}
		if humanoid.Health <= 0 then
			table.insert(notes, "le personnage est mort en chemin")
		elseif os.clock() >= deadline then
			table.insert(notes, "délai dépassé")
		end
		if not hasPath then
			table.insert(notes, "aucun chemin trouvé, marche en ligne droite")
		end

		local distance = ((position - target) * flat).Magnitude
		return {
			text = string.format(
				"Position %.1f, %.1f, %.1f, à %.1f studs de la cible%s",
				position.X,
				position.Y,
				position.Z,
				distance,
				if #notes > 0 then " (" .. table.concat(notes, " ; ") .. ")" else ""
			),
			distance = distance,
			arrived = distance <= 4 and humanoid.Health > 0,
		}
	end

	function methods.place_info()
		local size = workspace.CurrentCamera.ViewportSize
		return {
			placeId = game.PlaceId,
			gameId = game.GameId,
			name = game.Name,
			width = math.round(size.X),
			height = math.round(size.Y),
		}
	end

	local STAGED = "RoVibeApercu"

	local function clearStage()
		local previous = workspace:FindFirstChild(STAGED)
		if previous then
			previous:Destroy()
		end
	end

	-- Lays a bank asset out high above the map, where nothing else is in the
	-- picture, for the app to photograph it.
	function methods.asset_stage(params)
		clearStage()
		local data = EncodingService:Base64Decode(buffer.fromstring(params.data))
		local roots = SerializationService:DeserializeInstancesAsync(data)
		if #roots == 0 then
			error("L'asset ne contient aucune instance", 0)
		end

		local count = 0
		for _, instance in collectInstances(roots) do
			count += 1
			if instance:IsA("BaseScript") then
				(instance :: any).Enabled = false
			end
		end

		local stage = Instance.new("Model")
		stage.Name = STAGED
		stage.Archivable = false
		for _, root in roots do
			root.Parent = stage
		end
		local visible = stage:FindFirstChildWhichIsA("BasePart", true) ~= nil
		if visible then
			stage:PivotTo(CFrame.new(0, 4000, 0))
		end
		stage.Parent = workspace

		return {
			path = "game.Workspace." .. STAGED,
			class = roots[1].ClassName,
			count = count,
			visible = visible,
		}
	end

	function methods.asset_unstage()
		clearStage()
		return "ok"
	end

	function methods.camera_get()
		local camera = workspace.CurrentCamera
		local size = camera.ViewportSize
		return {
			cframe = { camera.CFrame:GetComponents() },
			focus = { camera.Focus:GetComponents() },
			fov = camera.FieldOfView,
			cameraType = camera.CameraType.Name,
			width = math.round(size.X),
			height = math.round(size.Y),
		}
	end

	function methods.camera_set(params)
		local camera = workspace.CurrentCamera
		-- The type first: a game's camera script takes a Custom camera back
		-- as soon as it is set, and must find it where it was.
		if params.cameraType then
			pcall(function()
				camera.CameraType = (Enum.CameraType :: any)[params.cameraType]
			end)
		end
		if params.fov then
			camera.FieldOfView = params.fov
		end
		camera.CFrame = CFrame.new(table.unpack(params.cframe))
		camera.Focus = CFrame.new(table.unpack(params.focus))
		return "ok"
	end

	-- Where something is and how big, for pointing a camera at it.
	local function extent(instance)
		if instance:IsA("Model") then
			local cframe, size = instance:GetBoundingBox()
			return cframe.Position, size.Magnitude / 2
		elseif instance:IsA("BasePart") then
			return instance.Position, instance.Size.Magnitude / 2
		elseif instance:IsA("Attachment") then
			return instance.WorldPosition, 4
		elseif instance:IsA("PVInstance") then
			return instance:GetPivot().Position, 8
		end
		return nil, nil
	end

	-- Points the camera at a target from a chosen angle. Returns the camera
	-- as it was, for the app to put it back once the picture is taken, and
	-- where it was put, to tell afterwards whether a game script moved it.
	function methods.frame_camera(params)
		local camera = workspace.CurrentCamera
		local before = methods.camera_get()

		local center, radius
		if params.target then
			local instance, resolveError = resolvePath(params.target)
			if not instance then
				error(resolveError, 0)
			end
			center, radius = extent(instance)
			if not center then
				error(getFullPath(instance) .. " n'a pas de position : vise un Model, une BasePart ou un Attachment", 0)
			end
		end
		if params.look_at then
			center = Vector3.new(params.look_at[1], params.look_at[2], params.look_at[3])
			radius = radius or 8
		end
		if not center then
			error("`frame` attend `target` (chemin) ou `look_at` ([x, y, z])", 0)
		end
		radius = math.max(radius, 1)

		local fov = if params.fov then math.clamp(params.fov, 1, 120) else camera.FieldOfView
		local position
		local elevation = math.clamp(params.elevation or 25, -89, 90)
		if params.position then
			position = Vector3.new(params.position[1], params.position[2], params.position[3])
		else
			local azimuth = math.rad(params.azimuth or 45)
			local pitch = math.rad(elevation)
			local distance = params.distance or radius / math.tan(math.rad(fov / 2)) * 1.2
			local direction = Vector3.new(math.sin(azimuth) * math.cos(pitch), math.sin(pitch), math.cos(azimuth) * math.cos(pitch))
			position = center + direction * distance
		end
		-- Straight down, "up" is undefined: the top of the picture faces -Z.
		local up = if elevation > 89 then Vector3.new(0, 0, -1) else Vector3.yAxis
		local cframe = CFrame.lookAt(position, center, up)

		if RunService:IsRunning() then
			camera.CameraType = Enum.CameraType.Scriptable
		end
		camera.FieldOfView = fov
		camera.CFrame = cframe
		camera.Focus = CFrame.new(center)
		return { before = before, expected = { cframe:GetComponents() } }
	end

	-- Whether the camera is still where frame_camera put it.
	function methods.camera_held(params)
		local expected = CFrame.new(table.unpack(params.expected))
		local current = workspace.CurrentCamera.CFrame
		return (current.Position - expected.Position).Magnitude < 0.05
			and current.LookVector:Dot(expected.LookVector) > 0.9999
	end

	-- What the user has selected in Studio, which is what they mean by
	-- "this" or "that part"; optionally selects instances instead.
	function methods.selection(params)
		local Selection = game:GetService("Selection")
		local lines = {}
		if params.select then
			local wanted, missing = {}, {}
			for _, path in params.select do
				local instance, resolveError = resolvePath(path)
				if instance then
					table.insert(wanted, instance)
				else
					table.insert(missing, path .. " (" .. tostring(resolveError) .. ")")
				end
			end
			Selection:Set(wanted)
			if #missing > 0 then
				table.insert(lines, "Introuvable, donc pas sélectionné : " .. table.concat(missing, ", "))
			end
		end

		local current = Selection:Get()
		if #current == 0 then
			table.insert(lines, "0 sélectionné : rien n'est sélectionné dans Studio. Demande à l'utilisateur de cliquer ce dont il parle dans l'explorateur ou la vue 3D, ou passe `select`.")
			return table.concat(lines, "\n")
		end
		table.insert(lines, string.format("%d sélectionné(s) :", #current))
		for index, instance in current do
			if index > 50 then
				table.insert(lines, string.format("… et %d autres", #current - 50))
				break
			end
			local line = "  " .. getFullPath(instance) .. " [" .. instance.ClassName .. "]"
			local center = extent(instance)
			if center then
				line ..= string.format(" à (%.1f, %.1f, %.1f)", center.X, center.Y, center.Z)
			end
			table.insert(lines, line)
		end
		return table.concat(lines, "\n")
	end

	-- Appends a titled list of findings, cut after `limit` items.
	local function report(lines, title, items, advice, limit)
		if #items == 0 then
			return false
		end
		table.insert(lines, string.format("✗ %d %s", #items, title))
		for index, item in items do
			if index > (limit or 8) then
				table.insert(lines, string.format("    … et %d autres", #items - (limit or 8)))
				break
			end
			table.insert(lines, "    " .. item)
		end
		if advice then
			table.insert(lines, "    → " .. advice)
		end
		return true
	end

	-- Checks that a model holds together: every part reachable from its
	-- PrimaryPart through welds or Motor6Ds, and nothing that keeps a joint
	-- from moving.
	function methods.rig_check(params)
		local model, resolveError = resolvePath(params.path)
		if not model then
			error(resolveError, 0)
		end
		if not model:IsA("Model") then
			error(getFullPath(model) .. " est un " .. model.ClassName .. ", pas un Model", 0)
		end

		local parts = {}
		for _, descendant in model:GetDescendants() do
			if descendant:IsA("BasePart") then
				table.insert(parts, descendant)
			end
		end
		local lines = { string.format("rig_check sur %s : %d pièce(s)", getFullPath(model), #parts) }
		if #parts == 0 then
			table.insert(lines, "✗ aucune BasePart : rien à assembler")
			return table.concat(lines, "\n")
		end

		local humanoid = model:FindFirstChildWhichIsA("Humanoid")
		local root = model.PrimaryPart or (humanoid and model:FindFirstChild("HumanoidRootPart"))
		if not root then
			table.insert(lines, "✗ pas de PrimaryPart : choisis la pièce dont tout dépend (torse, châssis, poignée) ; chaque vérification part d'elle")
			return table.concat(lines, "\n")
		end
		table.insert(lines, "✓ racine : " .. root.Name)

		-- Welds and every JointInstance (Weld, Motor6D, Snap…) hold parts
		-- together. Hinges and ropes are physics, not links.
		local links, motors, driven, broken = {}, {}, {}, {}
		local function link(a, b)
			links[a] = links[a] or {}
			links[b] = links[b] or {}
			table.insert(links[a], b)
			table.insert(links[b], a)
		end
		for _, descendant in model:GetDescendants() do
			if descendant:IsA("WeldConstraint") or descendant:IsA("JointInstance") then
				local a, b = (descendant :: any).Part0, (descendant :: any).Part1
				if a == nil or b == nil then
					table.insert(broken, getFullPath(descendant) .. " : Part0 ou Part1 vide")
				elseif a == b then
					table.insert(broken, getFullPath(descendant) .. " : relie une pièce à elle-même")
				else
					link(a, b)
					if descendant:IsA("Motor6D") then
						table.insert(motors, descendant)
						driven[b] = (driven[b] or 0) + 1
					end
				end
			end
		end

		local reached, count, queue = { [root] = true }, 1, { root }
		while #queue > 0 do
			local current = table.remove(queue)
			for _, other in links[current] or {} do
				if not reached[other] then
					reached[other] = true
					count += 1
					table.insert(queue, other)
				end
			end
		end

		local loose, anchoredLimbs = {}, {}
		for _, part in parts do
			if not reached[part] then
				table.insert(
					loose,
					getFullPath(part)
						.. if part.Anchored then " (ancrée : reste sur place au lieu de suivre)" else " (non ancrée : tombe dès que la physique démarre)"
				)
			elseif part ~= root and part.Anchored and #motors > 0 then
				table.insert(anchoredLimbs, getFullPath(part))
			end
		end

		local stuck, doubled = {}, {}
		for _, motor in motors do
			if motor.C0 == CFrame.identity and motor.C1 == CFrame.identity then
				table.insert(stuck, string.format("%s : %s collée au centre de %s", getFullPath(motor), motor.Part1.Name, motor.Part0.Name))
			end
		end
		for part, drivers in driven do
			if drivers > 1 then
				table.insert(doubled, string.format("%s est Part1 de %d Motor6D", getFullPath(part), drivers))
			end
		end

		-- All anchored and nothing animated: a fixed prop, which holds by
		-- being anchored and has nothing to link.
		local allAnchored = true
		for _, part in parts do
			if not part.Anchored then
				allAnchored = false
				break
			end
		end
		if allAnchored and #motors == 0 and not humanoid then
			table.insert(lines, "✓ modèle fixe : toutes les pièces sont ancrées et aucun Motor6D. Rien à relier tant qu'il ne doit ni bouger ni être déplacé par la physique ; pour l'animer, désancre tout sauf la racine et soude le reste.")
			loose = {}
		end

		local found = false
		found = report(lines, "pièce(s) non reliée(s) à la racine", loose, "soude chaque pièce à la racine (WeldConstraint), puis remplace par un Motor6D la soudure des pièces qui doivent bouger") or found
		found = report(lines, "joint(s) cassé(s)", broken, "supprime-les ou répare-les : un joint sans pièce ne tient rien") or found
		found = report(lines, "pièce(s) ancrée(s) dans un modèle animé", anchoredLimbs, "une pièce ancrée ne suit pas son Motor6D : n'ancre que la racine, et seulement pour une machine fixe") or found
		found = report(lines, "Motor6D avec C0 et C1 à l'identité", stuck, "place le pivot à la charnière réelle : C0 = Part0.CFrame:Inverse() * pivot, C1 = Part1.CFrame:Inverse() * pivot") or found
		found = report(lines, "pièce(s) pilotée(s) par plusieurs Motor6D", doubled, "un seul Motor6D par pièce animée") or found

		if humanoid then
			local animator = humanoid:FindFirstChildWhichIsA("Animator")
			table.insert(lines, string.format("Humanoid %s, %s Animator%s", humanoid.RigType.Name, if animator then "avec" else "sans", if root.Anchored then ", racine ANCRÉE : le personnage ne bougera pas" else ""))
		end
		table.insert(lines, string.format("%d Motor6D, %d/%d pièce(s) reliée(s) à la racine", #motors, count, #parts))
		if not found then
			table.insert(lines, "✓ aucun problème d'assemblage")
		end
		return table.concat(lines, "\n")
	end

	-- The world-aligned box around parts, from each part's own corners.
	-- Model:GetBoundingBox follows the PrimaryPart's orientation, which is
	-- wrong as soon as that part is turned (a cylinder lies along X).
	local function worldBox(parts)
		local low, high = Vector3.one * math.huge, -Vector3.one * math.huge
		local lowest = nil
		for _, part in parts do
			local cframe, half = part.CFrame, part.Size / 2
			local reach = Vector3.new(
				math.abs(cframe.RightVector.X) * half.X + math.abs(cframe.UpVector.X) * half.Y + math.abs(cframe.LookVector.X) * half.Z,
				math.abs(cframe.RightVector.Y) * half.X + math.abs(cframe.UpVector.Y) * half.Y + math.abs(cframe.LookVector.Y) * half.Z,
				math.abs(cframe.RightVector.Z) * half.X + math.abs(cframe.UpVector.Z) * half.Y + math.abs(cframe.LookVector.Z) * half.Z
			)
			local partLow = cframe.Position - reach
			if not lowest or partLow.Y < lowest.low.Y then
				lowest = { part = part, low = partLow, center = cframe.Position }
			end
			low = low:Min(partLow)
			high = high:Max(cframe.Position + reach)
		end
		return (low + high) / 2, high - low, lowest
	end

	-- What sits under the lowest part of a model: how far it floats above,
	-- or sinks into, the first thing below it.
	local function grounding(instance, size, lowest)
		if size.X * size.Z > 250000 then
			return "(trop étendu pour parler d'appui : sol ou plateau)"
		end
		local params = RaycastParams.new()
		params.FilterType = Enum.RaycastFilterType.Exclude
		params.FilterDescendantsInstances = { instance }
		local bottom = lowest.low.Y
		local from = Vector3.new(lowest.center.X, lowest.center.Y + 1, lowest.center.Z)
		local hit = workspace:Raycast(from, Vector3.new(0, -(from.Y - bottom) - 200, 0), params)
		if not hit then
			return "⚠ rien en dessous sur 200 studs : il flotte dans le vide"
		end
		local gap = bottom - hit.Position.Y
		local under = getFullPath(hit.Instance)
		-- Planted a little into terrain is how trees and rocks are meant to
		-- sit; into a part, it shows.
		local sinkTolerance = if hit.Instance:IsA("Terrain") then 1.5 else 0.2
		if gap > 0.2 then
			return string.format("⚠ flotte à %.2f studs au-dessus de %s", gap, under)
		elseif -gap > sinkTolerance then
			return string.format("⚠ s'enfonce de %.2f studs dans %s", -gap, under)
		end
		return "✓ posé sur " .. under .. (if gap < -0.2 then string.format(" (enfoncé de %.2f studs)", -gap) else "")
	end

	-- Size, make-up and placement of models or parts, before building with
	-- them or after placing them.
	function methods.asset_check(params)
		local paths = params.paths or { params.path }
		local lines = {}
		for _, path in paths do
			local instance, resolveError = resolvePath(path)
			if not instance then
				table.insert(lines, tostring(path) .. " : " .. tostring(resolveError))
				continue
			end
			if instance:IsA("Model") then
				local partList = {}
				local parts, meshes, unanchored, joints, scripts = 0, 0, 0, 0, 0
				for _, descendant in instance:GetDescendants() do
					if descendant:IsA("BasePart") then
						table.insert(partList, descendant)
						parts += 1
						if descendant:IsA("MeshPart") or descendant:IsA("UnionOperation") then
							meshes += 1
						end
						if not descendant.Anchored then
							unanchored += 1
						end
					elseif descendant:IsA("JointInstance") or descendant:IsA("WeldConstraint") then
						joints += 1
					elseif descendant:IsA("BaseScript") then
						scripts += 1
					end
				end
				if parts == 0 then
					table.insert(lines, getFullPath(instance) .. " [Model] : ✗ aucune BasePart, rien à voir")
					continue
				end
				local center, size, lowest = worldBox(partList)
				table.insert(lines, string.format("%s [Model] %.1f × %.1f × %.1f studs, centre (%.1f, %.1f, %.1f)", getFullPath(instance), size.X, size.Y, size.Z, center.X, center.Y, center.Z))
				table.insert(lines, string.format("  %d pièce(s) dont %d mesh/union, %d joint(s), %d non ancrée(s), %d script(s)", parts, meshes, joints, unanchored, scripts))
				local primary = instance.PrimaryPart
				if not primary then
					table.insert(lines, "  ⚠ pas de PrimaryPart : PivotTo et les déplacements par script tournent autour d'un point arbitraire")
				else
					-- What turning or moving the model follows is its pivot,
					-- not how its PrimaryPart happens to be oriented.
					local upY = instance:GetPivot().UpVector.Y
					if upY < -0.99 then
						table.insert(lines, "  ⚠ pivot à l'envers : PivotTo retournera le modèle")
					elseif upY < 0.99 then
						table.insert(lines, string.format("  ⚠ pivot penché de %.0f° : PivotTo vers une orientation droite penchera le modèle (règle PivotOffset)", math.deg(math.acos(math.clamp(upY, -1, 1)))))
					else
						table.insert(lines, "  ✓ PrimaryPart " .. primary.Name .. ", pivot droit")
					end
				end
				if unanchored > 0 and joints == 0 and parts > 1 then
					table.insert(lines, string.format("  ⚠ %d pièce(s) non ancrée(s) sans aucun joint : elles tomberont séparément", unanchored))
				end
				if instance:IsDescendantOf(workspace) then
					table.insert(lines, "  " .. grounding(instance, size, lowest))
				end
			elseif instance:IsA("BasePart") then
				table.insert(lines, string.format("%s [%s] %.1f × %.1f × %.1f studs à (%.1f, %.1f, %.1f), %s", getFullPath(instance), instance.ClassName, instance.Size.X, instance.Size.Y, instance.Size.Z, instance.Position.X, instance.Position.Y, instance.Position.Z, if instance.Anchored then "ancrée" else "NON ANCRÉE"))
				if instance:IsDescendantOf(workspace) then
					local _, size, lowest = worldBox({ instance })
					table.insert(lines, "  " .. grounding(instance, size, lowest))
				end
			else
				table.insert(lines, getFullPath(instance) .. " est un " .. instance.ClassName .. " : asset_check lit les Models et les BaseParts")
			end
		end
		return table.concat(lines, "\n")
	end

	-- Measures an interface as it is drawn: what sticks out of the screen,
	-- text that doesn't fit, buttons too small to tap or covering each other.
	function methods.ui_check(params)
		local guiRoots = {}
		if params.path then
			local instance, resolveError = resolvePath(params.path)
			if not instance then
				error(resolveError, 0)
			end
			table.insert(guiRoots, instance)
		elseif RunService:IsRunning() then
			local player = game:GetService("Players").LocalPlayer
			if not player then
				error("Pendant un test, l'interface se mesure dans le client (context: \"client\")", 0)
			end
			table.insert(guiRoots, player:WaitForChild("PlayerGui", 5))
		else
			table.insert(guiRoots, game:GetService("StarterGui"))
		end

		local viewport = workspace.CurrentCamera.ViewportSize
		local inset = game:GetService("GuiService"):GetGuiInset()
		local offscreen, clipped, tiny, overlapping, fixed = {}, {}, {}, {}, {}
		local measured, buttons = 0, {}

		local function visibleUnder(object)
			local current = object
			while current and current:IsA("GuiObject") do
				if not current.Visible then
					return false
				end
				current = current.Parent
			end
			if current and current:IsA("LayerCollector") then
				return (current :: any).Enabled ~= false
			end
			return current ~= nil
		end

		local function clippedByParent(object)
			local current = object.Parent
			while current and current:IsA("GuiObject") do
				if current.ClipsDescendants or current:IsA("ScrollingFrame") then
					return true
				end
				current = current.Parent
			end
			return false
		end

		for _, guiRoot in guiRoots do
			for _, object in guiRoot:GetDescendants() do
				if not object:IsA("GuiObject") or not visibleUnder(object) then
					continue
				end
				local screen = object:FindFirstAncestorWhichIsA("ScreenGui")
				if not screen then
					continue
				end
				local position, size = object.AbsolutePosition, object.AbsoluteSize
				if size.X <= 0 or size.Y <= 0 then
					continue
				end
				measured += 1
				-- AbsolutePosition starts below the top bar unless the
				-- ScreenGui ignores it.
				local area = if screen.IgnoreGuiInset then viewport else viewport - inset
				if not clippedByParent(object) then
					local over = math.max(-position.X, -position.Y, position.X + size.X - area.X, position.Y + size.Y - area.Y)
					if over > 2 then
						table.insert(offscreen, string.format("%s dépasse de %d px", getFullPath(object), math.round(over)))
					end
				end
				if object:IsA("TextLabel") or object:IsA("TextButton") or object:IsA("TextBox") then
					local text = object :: any
					if not text.TextScaled and text.Text ~= "" and not text.TextFits then
						table.insert(clipped, string.format("%s : « %s »", getFullPath(object), string.sub(text.Text, 1, 40)))
					end
				end
				if object:IsA("GuiButton") then
					table.insert(buttons, object)
					if size.X < 32 or size.Y < 32 then
						table.insert(tiny, string.format("%s : %d × %d px", getFullPath(object), math.round(size.X), math.round(size.Y)))
					end
				end
				if object.Parent == screen and object.Size.X.Scale == 0 and object.Size.X.Offset > 400 then
					table.insert(fixed, string.format("%s : %d px de large, fixe", getFullPath(object), object.Size.X.Offset))
				end
			end
		end

		-- Two buttons stacked on each other: the one drawn on top swallows the
		-- clicks meant for the other.
		for i = 1, math.min(#buttons, 150) do
			for j = i + 1, math.min(#buttons, 150) do
				local a, b = buttons[i], buttons[j]
				if a:IsAncestorOf(b) or b:IsAncestorOf(a) then
					continue
				end
				if a:FindFirstAncestorWhichIsA("ScreenGui") ~= b:FindFirstAncestorWhichIsA("ScreenGui") then
					continue
				end
				local left = math.max(a.AbsolutePosition.X, b.AbsolutePosition.X)
				local top = math.max(a.AbsolutePosition.Y, b.AbsolutePosition.Y)
				local right = math.min(a.AbsolutePosition.X + a.AbsoluteSize.X, b.AbsolutePosition.X + b.AbsoluteSize.X)
				local bottom = math.min(a.AbsolutePosition.Y + a.AbsoluteSize.Y, b.AbsolutePosition.Y + b.AbsoluteSize.Y)
				if right > left and bottom > top then
					local shared = (right - left) * (bottom - top)
					local smaller = math.min(a.AbsoluteSize.X * a.AbsoluteSize.Y, b.AbsoluteSize.X * b.AbsoluteSize.Y)
					if smaller > 0 and shared / smaller > 0.3 then
						table.insert(overlapping, getFullPath(a) .. " et " .. getFullPath(b))
					end
				end
			end
		end

		local lines = { string.format("ui_check : %d élément(s) visibles mesurés, écran %d × %d px", measured, math.round(viewport.X), math.round(viewport.Y)) }
		if measured == 0 then
			table.insert(lines, "Rien de mesuré : aucune interface visible ici. En édition, StarterGui doit avoir ShowDevelopmentGui ; pendant un test, mesure en contexte client.")
			return table.concat(lines, "\n")
		end
		local found = false
		found = report(lines, "élément(s) hors de l'écran", offscreen, "ancre-le (AnchorPoint) et positionne-le en Scale plutôt qu'en pixels") or found
		found = report(lines, "texte(s) coupé(s)", clipped, "agrandis la zone, raccourcis le texte, ou TextScaled avec un UITextSizeConstraint") or found
		found = report(lines, "bouton(s) de moins de 32 px", tiny, "au moins 44 px pour un doigt sur téléphone") or found
		found = report(lines, "paire(s) de boutons qui se recouvrent", overlapping, "celui du dessus prend les clics de l'autre : écarte-les ou règle ZIndex/Visible") or found
		if #fixed > 0 then
			report(lines, "cadre(s) de largeur fixe (indicatif)", fixed, "plus large que la moitié d'un téléphone : préfère Scale ou un UIScale")
		end
		if not found then
			table.insert(lines, "✓ aucun problème mesuré")
		end
		return table.concat(lines, "\n")
	end

	function methods.focus_camera(params)
		local instance, resolveError = resolvePath(params.path)
		if not instance then
			error(resolveError, 0)
		end

		local center, size
		if instance:IsA("Model") then
			center, size = instance:GetBoundingBox()
		elseif instance:IsA("BasePart") then
			center, size = instance.CFrame, instance.Size
		else
			error("focus attend un Model ou une BasePart", 0)
		end

		-- Far enough for the bounding sphere to fit the vertical field of view,
		-- seen from a three-quarter angle so depth stays readable.
		local camera = workspace.CurrentCamera
		local radius = math.max(size.Magnitude / 2, 2)
		local distance = radius / math.tan(math.rad(camera.FieldOfView / 2)) * 1.15
		local direction = Vector3.new(1, 0.6, 1).Unit

		camera.CFrame = CFrame.lookAt(center.Position + direction * distance, center.Position)
		camera.Focus = CFrame.new(center.Position)
		return "ok"
	end

	-- Sync servers whose changes Studio must show before applying them.
	function methods.protect_ports(params)
		if not env.protectedPorts then
			error("Ce plugin ne sait pas protéger un projet : mets-le à jour", 0)
		end
		table.clear(env.protectedPorts)
		for _, port in params.ports or {} do
			env.protectedPorts[tostring(port)] = true
		end
		return "ok"
	end

	function methods.sync_connect(params)
		local syncController = env.getSyncController()
		if not syncController then
			error("Sync is only available in the edit DataModel", 0)
		end
		-- An older plugin's connect takes no token and ignores it.
		syncController.connect(params.host or "localhost", tostring(params.port), params.token)
		return "connecting"
	end

	-- Where this window's sync stands: connected, to which port, and up to
	-- which message of the server. `supported` is false for a plugin that
	-- predates the report.
	function methods.sync_status()
		local syncController = env.getSyncController()
		if not syncController or not syncController.status then
			return { supported = false }
		end
		local status = syncController.status()
		status.supported = true
		return status
	end

	-- The RoVibe project the place is stamped for: `ServerStorage.RoVibe`,
	-- attribute `ProjectId`. With `set`, stamps it; with `set` and an empty
	-- string, removes the stamp.
	function methods.project_marker(params)
		local ServerStorage = game:GetService("ServerStorage")
		local marker = ServerStorage:FindFirstChild("RoVibe")
		if params.set ~= nil then
			if params.set == "" then
				if marker then
					marker:SetAttribute("ProjectId", nil)
				end
			else
				if not marker then
					marker = Instance.new("Configuration")
					marker.Name = "RoVibe"
					marker.Parent = ServerStorage
				end
				marker:SetAttribute("ProjectId", params.set)
			end
		end
		marker = ServerStorage:FindFirstChild("RoVibe")
		local id = marker and marker:GetAttribute("ProjectId")
		return { projectId = if type(id) == "string" and id ~= "" then id else nil }
	end

	function methods.sync_disconnect()
		local syncController = env.getSyncController()
		if syncController then
			syncController.disconnect()
		end
		return "disconnected"
	end

	-- Services whose scripts are worth mirroring to files when a place is imported.
	local CODE_SERVICES = {
		"Workspace",
		"ReplicatedFirst",
		"ReplicatedStorage",
		"ServerScriptService",
		"ServerStorage",
		"StarterGui",
		"StarterPack",
		"StarterPlayer",
	}

	function methods.export_scripts()
		local ScriptEditorService = game:GetService("ScriptEditorService")
		local scripts, skipped = {}, {}
		local ambiguous = {}

		-- The sync server matches instances to files by name, so two siblings
		-- with one name can't both be mirrored safely.
		local function isAmbiguous(instance)
			local cached = ambiguous[instance]
			if cached == nil then
				local lowered = string.lower(instance.Name)
				local count = 0
				for _, sibling in instance.Parent:GetChildren() do
					if string.lower(sibling.Name) == lowered then
						count += 1
					end
				end
				cached = count > 1
				ambiguous[instance] = cached
			end
			return cached
		end

		for _, serviceName in CODE_SERVICES do
			local service = game:FindService(serviceName)
			if not service then
				continue
			end

			for _, instance in service:GetDescendants() do
				if not instance:IsA("LuaSourceContainer") then
					continue
				end

				local chain = {}
				local problem = nil
				local current = instance
				while current ~= service do
					if isAmbiguous(current) then
						problem = string.format("plusieurs instances nommées « %s » au même endroit", current.Name)
						break
					end
					table.insert(chain, 1, { name = current.Name, class = current.ClassName })
					current = current.Parent
				end

				if problem then
					table.insert(skipped, getFullPath(instance) .. " : " .. problem)
					continue
				end

				-- The editor may hold changes the Source property doesn't have yet.
				local ok, text = pcall(ScriptEditorService.GetEditorSource, ScriptEditorService, instance)
				table.insert(scripts, {
					service = serviceName,
					chain = chain,
					source = if ok then text else (instance :: any).Source,
					children = #instance:GetChildren(),
					disabled = instance:IsA("BaseScript") and not (instance :: any).Enabled,
					runContext = if instance:IsA("Script") then (instance :: any).RunContext.Name else nil,
				})
			end
		end

		return { scripts = scripts, skipped = skipped, place = game.Name, placeId = game.PlaceId }
	end

	return methods
end
