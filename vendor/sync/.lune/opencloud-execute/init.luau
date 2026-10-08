--!optimize 2
local process = require("@lune/process")
local net = require("@lune/net")
local serde = require("@lune/serde")

local API_KEY_ENV_VAR = "RBX_API_KEY"

local MAX_PAYLOAD_SIZE = 4_000_000 -- 4mb
local MAX_INPUT_SIZE = 104_857_600 -- 100 MiB
local MAX_EXECUTION_TIME = 300 -- 5 minutes

--- `await_finish` exponentially backs off querying the status of the task
--- but will stop when it's yielding this number of seconds and no longer
--- increase the time to yield
local MAX_QUERY_YIELD = 16

local BASE_URL = "https://apis.roblox.com/cloud/v2"
local TASK_CREATE_LATEST = `{BASE_URL}/universes/%s/places/%s/luau-execution-session-tasks`
local TASK_CREATE_VERSIONED = `{BASE_URL}/universes/%s/places/%s/versions/%s/luau-execution-session-tasks`
local BINARY_INPUT_CREATE = `{BASE_URL}/universes/%s/luau-execution-session-task-binary-inputs`

local USER_AGENT = `Dekkonot/Lune-OpenCloud-Execution 1.0.0; {_VERSION}`

local RATE_LIMIT_HEADER = "x-ratelimit-reset"

local ID_PATTERN = "([%w%-]+)/tasks/([%w%-]+)$"

local api_key_override: string?

local function api_key(): string
	if api_key_override then
		return api_key_override
	else
		local api_key = process.env[API_KEY_ENV_VAR]
		if not api_key then
			error(`no Open Cloud API key specified (consider specifying one with the env var {API_KEY_ENV_VAR})`)
		else
			return api_key
		end
	end
end

--[[
	Represents the state of a Luau execution task.
	In general, `STATE_UNSPECIFIED` should never occur.
]]
export type LuauTaskState = "STATE_UNSPECIFIED" | "QUEUED" | "PROCESSING" | "CANCELLED" | "COMPLETE" | "FAILED"

--[[
	Represents a Luau execution task.
	A table of this structure should be passed to functions in this module.
]]
export type LuauExecutionTask = {
	--- The internal resource path for this task.
	path: string,
	--- The user ID of the user that created this task.
	user: string,
	--- The state of the task. This will only update when a new task object
	--- is returned after using `query_status`.
	state: LuauTaskState,
	--- The timeout of the task, specified in seconds.
	timeout: string,

	--- The path to the binary input used for this task, if one was provided.
	binaryInput: string,
	--- Whether or not the output will include a binary blob
	enableBinaryOutput: boolean,
	--- A path to the binary blob of the output
	binaryOutputUri: string,

	-- Not available upon creation, only querying

	--- The time the task was created, formatted as an RFC 3339 timestamp.
	createTime: string?,
	--- The time the task was last updated (e.g. status changed), formatted as an RFC 3339 timestamp.
	updateTime: string?,
	output: {
		results: { unknown },
	}?,
	error: TaskError?,
	script: string?,
}

--[[
	Represents one of the types that a log from Roblox can be.

	- `OUTPUT` is a standard message (e.g. from `print`)
	- `WARNING` is a warning (e.g. from `warn`)
	- `ERROR` is an error (e.g. from `error`)
	- `INFO` is the blue text that shows up in Studio's output
	- `MESSAGE_TYPE_UNSPECIFIED` should never occur
]]
export type MessageType = "MESSAGE_TYPE_UNSPECIFIED" | "OUTPUT" | "INFO" | "WARNING" | "ERROR"

--[[
	Represents a structured log from Roblox. 
	This includes the log message itself along with the type and the time it
	was sent.
]]
export type StructuredLog = {
	--- This log's message
	message: string,
	--- The time that the log was emitted, as an RFC 3339 timestamp
	createTime: string,
	--- The type that this log is
	messageType: MessageType,
}

type LuauExecutionSessionTaskLogs = {
	luauExecutionSessionTaskLogs: {
		{
			path: string,
			messages: { string },
			structuredMessages: { StructuredLog },
		}
	},
	nextPageToken: string,
}

--[[
	Represents one of the types of errors that a task can run into.

	- `SCRIPT_ERROR` - The task's script raised an unhandled error.
	- `DEADLINE_EXCEEDED` - The task exceeded the maximum allowed execution duration.
	- `OUTPUT_SIZE_LIMIT_EXCEEDED` - The task's outputs were too large.
	- `INTERNAL_ERROR` - The Task failed due to an internal system error.
	- `ERROR_CODE_UNSPECIFIED` - Should never occur
]]
export type TaskErrorType =
	| "ERROR_CODE_UNSPECIFIED"
	| "SCRIPT_ERROR"
	| "DEADLINE_EXCEEDED"
	| "OUTPUT_SIZE_LIMIT_EXCEEDED"
	| "INTERNAL_ERROR"

--[[
	Represents an error that a task ran into.
]]
export type TaskError = {
	--- What type of error occured
	code: TaskErrorType,
	--- A message providing more information about the error that occured.
	message: string,
}

--- A response from uploading a binary input
type InputUploadResponse = {
	path: string,
	size: number,
	uploadUri: string,
}

function upload_input(input: buffer | string, universe_id: string): string
	local input_size = if typeof(input) == "buffer" then buffer.len(input) else #input
	if input_size > MAX_INPUT_SIZE then
		error(
			`failed to upload input: input is too large (max size is {MAX_INPUT_SIZE} bytes, your data is {input_size} bytes)`
		)
	end
	local request = {
		size = input_size,
	}

	local result = net.request({
		method = "POST",
		url = string.format(BINARY_INPUT_CREATE, universe_id),
		body = serde.encode("json", request),
		headers = {
			["User-Agent"] = USER_AGENT,
			["Content-Type"] = "application/json",
			["X-API-Key"] = api_key(),
		},
	})
	if not result.ok then
		if result.statusCode == 429 then
			error(
				`failed to upload input: {result.statusCode} {result.statusMessage}, try again in {result.headers[RATE_LIMIT_HEADER]}s`
			)
		else
			error(`failed to upload input: {result.statusCode} {result.statusMessage}. body from error:\n{result.body}`)
		end
	end
	local response: InputUploadResponse = serde.decode("json", result.body)

	local result_2 = net.request({
		method = "PUT",
		url = response.uploadUri,
		body = input,
		headers = {
			["User-Agent"] = USER_AGENT,
			["Content-Type"] = "application/octet-stream",
			["X-API-Key"] = api_key(),
		},
	})
	if not result_2.ok then
		error(
			`failed to upload input: {result_2.statusCode} {result_2.statusMessage}. body from error:\n{result_2.body}`
		)
	end

	return response.path
end

local function make_task(
	url: string,
	script: string,
	timeout: number?,
	enable_binary_output: boolean?,
	binary_input: string?
): LuauExecutionTask
	local request = {
		script = script,
		timeout = `{timeout}s`,
		enableBinaryOutput = enable_binary_output or false,
		binaryInput = binary_input,
	}

	local body = serde.encode("json", request)
	if #body > MAX_PAYLOAD_SIZE then
		error("failed to create task: the final size of the request is too large for Roblox to accept")
	end

	local result = net.request({
		method = "POST",
		url = url,
		body = body,
		headers = {
			["User-Agent"] = USER_AGENT,
			["Content-Type"] = "application/json",
			["X-API-Key"] = api_key(),
		},
	})

	if not result.ok then
		if result.statusCode == 429 then
			error(
				`failed to create task: {result.statusCode} {result.statusMessage}, try again in {result.headers[RATE_LIMIT_HEADER]}s`
			)
		else
			error(`failed to create task: {result.statusCode} {result.statusMessage}. body from error:\n{result.body}`)
		end
	end
	local response: LuauExecutionTask = serde.decode("json", result.body)

	return response
end

function get_logs(
	task: LuauExecutionTask,
	view: "VIEW_UNSPECIFIED" | "FLAT" | "STRUCTURED"
): LuauExecutionSessionTaskLogs
	local result = net.request({
		method = "GET",
		url = `{BASE_URL}/{task.path}/logs`,
		headers = {
			["User-Agent"] = USER_AGENT,
			["X-API-Key"] = api_key(),
		},
		query = {
			["view"] = view,
		},
	})
	if not result.ok then
		if result.statusCode == 429 then
			error(
				`failed to get task logs: {result.statusCode} {result.statusMessage}, try again in {result.headers[RATE_LIMIT_HEADER]}s`
			)
		else
			error(
				`failed to get task logs: {result.statusCode} {result.statusMessage}. body from error:\n{result.body}`
			)
		end
	end

	return serde.decode("json", result.body)
end

function query_status(task: LuauExecutionTask): LuauExecutionTask
	local result = net.request({
		method = "GET",
		url = `{BASE_URL}/{task.path}`,
		headers = {
			["User-Agent"] = USER_AGENT,
			["X-API-Key"] = api_key(),
		},
	})

	if not result.ok then
		if result.statusCode == 429 then
			error(
				`failed to check task status: {result.statusCode} {result.statusMessage}, try again in {result.headers[RATE_LIMIT_HEADER]}s`
			)
		else
			error(
				`failed to check task status: {result.statusCode} {result.statusMessage}. body from error:\n{result.body}`
			)
		end
	end

	return serde.decode("json", result.body)
end

local luau_execute = {}

--[[
    Creates a new LuauExecutionTask for the provided universe ID and place ID.
    The created task runs on the latest version of the specified place.
]]
function luau_execute.create_task_latest(
	universe_id: string,
	place_id: string,
	script: buffer | string,
	timeout: number?,
	enable_binary_output: boolean?,
	binary_input: (buffer | string)?
): LuauExecutionTask
	timeout = timeout or MAX_EXECUTION_TIME
	if timeout < 1 or timeout > MAX_EXECUTION_TIME then
		error(`specified timeout is outside of acceptable range [1, {MAX_EXECUTION_TIME}]`)
	end
	if type(script) == "buffer" then
		script = buffer.tostring(script)
	end

	local input_path
	if binary_input then
		input_path = upload_input(binary_input, universe_id)
	end

	return make_task(
		string.format(TASK_CREATE_LATEST, universe_id, place_id),
		script :: string,
		timeout :: number,
		enable_binary_output,
		input_path
	)
end

--[[
    Creates a new LuauExecutionTask for the provided universe ID and place ID.
    The created task runs on the provided version of the place.
]]
function luau_execute.create_task_versioned(
	universe_id: string,
	place_id: string,
	version: string,
	script: buffer | string,
	timeout: number?,
	enable_binary_output: boolean?,
	binary_input: (buffer | string)?
): LuauExecutionTask
	timeout = timeout or MAX_EXECUTION_TIME
	if timeout < 1 or timeout > MAX_EXECUTION_TIME then
		error(`specified timeout is outside of acceptable range [1, {MAX_EXECUTION_TIME}]`)
	end
	if type(script) == "buffer" then
		script = buffer.tostring(script)
	end

	local input_path
	if binary_input then
		input_path = upload_input(binary_input, universe_id)
	end

	return make_task(
		string.format(TASK_CREATE_VERSIONED, universe_id, place_id, version),
		script :: string,
		timeout :: number,
		enable_binary_output,
		input_path
	)
end

--[[
	Returns the current status of the provided task. If this is `PROCESSING`,
	the task is not yet done.
]]
function luau_execute.check_status(task: LuauExecutionTask): LuauTaskState
	return query_status(task).state
end

--[[
	Yields until the provided task has been finished.

	When the task is done, returns whether it completed successfully
	as a boolean where `true` indicates success.
]]
function luau_execute.await_finish(task: LuauExecutionTask, timeout: number?): boolean
	timeout = timeout or MAX_EXECUTION_TIME
	local task_lib = require("@lune/task")
	local now = os.clock()
	local yield = 0.1

	local status: LuauTaskState = luau_execute.check_status(task)
	if status ~= "PROCESSING" then
		return status == "COMPLETE"
	end

	repeat
		task_lib.wait(yield)
		yield = math.min(yield * 2, MAX_QUERY_YIELD)
		status = luau_execute.check_status(task)
	until (status :: LuauTaskState) ~= "PROCESSING" or os.clock() - now >= timeout

	return (status :: LuauTaskState) == "COMPLETE"
end

--[[
	Gets a list of logs from the provided task. The returned array is
	just a list of literal strings emitted, without any data on what type.
]]
function luau_execute.get_flat_logs(task: LuauExecutionTask): { string }
	local logs = get_logs(task, "FLAT")

	-- TODO: Pagination
	return logs.luauExecutionSessionTaskLogs[1].messages
end

--[[
	Gets structured log data from the provided task. The returned array
	contains data on what type a log is and when it was created.
]]
function luau_execute.get_structured_logs(task: LuauExecutionTask): { StructuredLog }
	local logs = get_logs(task, "STRUCTURED")

	-- TODO: Pagination
	return logs.luauExecutionSessionTaskLogs[1].structuredMessages
end

--[[
	Returns the binary output returned by the provided task, if it exists.

	This function will error if binary output was not enabled when creating
	the task.
]]
function luau_execute.get_binary_output(task: LuauExecutionTask): buffer
	local status = query_status(task)
	local result = net.request({
		method = "GET",
		url = status.binaryOutputUri,
		headers = {
			["User-Agent"] = USER_AGENT,
			["X-API-Key"] = api_key(),
		},
	})

	if not result.ok then
		if result.statusCode == 429 then
			error(
				`failed to fetch input: {result.statusCode} {result.statusMessage}, try again in {result.headers[RATE_LIMIT_HEADER]}s`
			)
		else
			error(`failed to upload input: {result.statusCode} {result.statusMessage}. body from error:\n{result.body}`)
		end
	end

	return buffer.fromstring(result.body)
end

--[[
	Returns the error for the provided task, if one exists. This function will
	return `nil` if the task did not error. 
]]
function luau_execute.get_error(task: LuauExecutionTask): TaskError?
	local status = query_status(task)
	if status.state ~= "PROCESSING" then
		return status.error
	else
		luau_execute.await_finish(task)
		status = query_status(task)
		return status.error
	end
end

--[[
	Gets the output from the provided task. These are the values returned
	by the script.

	Only values that survive a JSON roundtrip will be returned.
]]
function luau_execute.get_output(task: LuauExecutionTask): { unknown }
	local status = query_status(task)
	if not status.output then
		return {}
	else
		return status.output.results
	end
end

--[[
	Returns the session ID for the provided task.
]]
function luau_execute.session_id(task: LuauExecutionTask): string
	local _, session_id = string.match(task.path, ID_PATTERN)
	return session_id
end

--[[
	Returns the task Id for the provided task.
]]
function luau_execute.task_id(task: LuauExecutionTask): string
	local task_id, _ = string.match(task.path, ID_PATTERN)
	return task_id
end

--[[
	Sets the API key to use for web requests. If set, this will
	override the API key read from the environment.
]]
function luau_execute.set_api_key(api_key: (buffer | string)?)
	if type(api_key) == "buffer" then
		api_key_override = buffer.tostring(api_key)
	elseif type(api_key) == "string" then
		api_key_override = api_key
	else
		api_key_override = nil
	end
end

return luau_execute
