--[[
	The Studio side of every Essaim tool. The app sends this chunk to the
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


	local function getFullPath(instance)
		if instance == game then
			return "game"
		end

		local names = {}
		local current = instance
		while current and current ~= game do
			table.insert(names, 1, current.Name)
			current = current.Parent
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
		for index, name in segments do
			local child = current:FindFirstChild(name)
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
		local available, loaded, compileError = pcall(loadstring, params.code, "=essaim")
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
			module.Name = "essaim"
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
			local ok, id = pcall(ChangeHistoryService.TryBeginRecording, ChangeHistoryService, "Essaim: run_luau")
			recording = if ok then id else nil
		end

		local finished, results = false, nil
		local thread = task.spawn(function()
			results = table.pack(xpcall(fn, function(message)
				-- Frames from this module are noise to whoever wrote the snippet.
				local lines = {}
				for _, line in string.split(debug.traceback(tostring(message), 2), "\n") do
					if line ~= "" and not string.find(line, "EssaimMethods", 1, true) then
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

		-- Execute*ModeAsync only returns when the test ends, so it runs detached.
		task.spawn(function()
			if params.mode == "run" then
				StudioTestService:ExecuteRunModeAsync({})
			else
				StudioTestService:ExecutePlayModeAsync({})
			end
		end)
		return "started"
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
			local ok, id = pcall(ChangeHistoryService.TryBeginRecording, ChangeHistoryService, "Essaim: asset_insert")
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
		if game:GetService("UserInputService").MouseBehavior ~= Enum.MouseBehavior.Default then
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

		return string.format(
			"Position %.1f, %.1f, %.1f, à %.1f studs de la cible%s",
			position.X,
			position.Y,
			position.Z,
			((position - target) * flat).Magnitude,
			if #notes > 0 then " (" .. table.concat(notes, " ; ") .. ")" else ""
		)
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

	function methods.camera_get()
		local camera = workspace.CurrentCamera
		local size = camera.ViewportSize
		return {
			cframe = { camera.CFrame:GetComponents() },
			focus = { camera.Focus:GetComponents() },
			width = math.round(size.X),
			height = math.round(size.Y),
		}
	end

	function methods.camera_set(params)
		local camera = workspace.CurrentCamera
		camera.CFrame = CFrame.new(table.unpack(params.cframe))
		camera.Focus = CFrame.new(table.unpack(params.focus))
		return "ok"
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

	function methods.sync_connect(params)
		local syncController = env.getSyncController()
		if not syncController then
			error("Sync is only available in the edit DataModel", 0)
		end
		syncController.connect(params.host or "localhost", tostring(params.port))
		return "connecting"
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
