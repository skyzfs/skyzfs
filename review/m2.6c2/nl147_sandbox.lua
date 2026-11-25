-- Next look at #147: run a channel program in Lua 5.2 with the globals
-- a ZFS channel program has and no others, against a model of a pool,
-- and print what `zfs program -j` would print in the locale C.
--
-- The sandbox (OpenZFS module/zfs/zcp.c, zcp_eval; module/lua/lbaselib.c
-- base_funcs; the same in zfs-2.2.2 and zfs-2.4.1): the base functions
-- assert collectgarbage error getmetatable ipairs next pairs rawequal
-- rawlen rawget rawset select setmetatable tonumber tostring type, and
-- the libraries coroutine, string (no dump) and table. No pcall,
-- xpcall, load, loadfile, dofile, print, require, io, os, debug.
--
-- The JSON (lib/libnvpair/libnvpair_json.c, nvlist_print_json_string):
-- each string through mbrtowc; in the locale C a byte above 0x7f is an
-- invalid sequence and the whole print stops there (the caller in
-- zfs_main.c ignores the error and exits 0). Control characters as
-- short escapes or \u00xx, 0x20 to 0x7f as they are.
--
-- usage: lua5.2 nl147_sandbox.lua PROGRAM SCENARIO OUT_JSON OUT_EXPECTED
-- SCENARIO names a pool model below. OUT_EXPECTED gets, for each member
-- and local property, the bytes the program was given (as numbers, by
-- string.byte, independent of the program's hex), one line each:
--   member<TAB>property<TAB>b1,b2,...
-- and a last line `instructions<TAB>N` (counted by a hook every 100).

local program_path, scenario, out_json, out_expected = ...

local f = assert(io.open(program_path, "rb"))
local text = f:read("*a")
f:close()

-- The pool model: datasets by name, each with children, system
-- properties {value, source} and user properties {name, value, source}.
local all_bytes = {}
for b = 0, 255 do all_bytes[#all_bytes + 1] = string.char(b) end
all_bytes = table.concat(all_bytes)

local function ds(name, system, user, children)
    return {name = name, system = system or {}, user = user or {}, children = children or {}}
end

local models = {}

-- Every single byte 1..255 as a user property's value, and as one value
-- of all 256 (NUL included, which ZFS cannot store; the program's hex
-- path is tried on it all the same), on the root; a child and a volume.
models.bytes = function()
    local user = {}
    for b = 1, 255 do
        user[#user + 1] = {string.format("u:b%03d", b), string.char(b), "tank"}
    end
    user[#user + 1] = {"u:all", all_bytes, "tank"}
    user[#user + 1] = {"u:empty", "", "tank"}
    user[#user + 1] = {"u:cafe", "caf\195\169", "tank"}
    user[#user + 1] = {"u:quote", "a\"b\\c/d", "tank"}
    user[#user + 1] = {"u:inherited", "\255", "tank/parent"}  -- not local
    user[#user + 1] = {"skyzfs:assist", "1", "tank"}         -- the agent's
    local system = {
        mountpoint = {"/srv/caf\195\169", "tank"},
        compression = {"lz4", "tank"},
        atime = {"off", "default"},
        quota = {0, "default"},
        recordsize = {131072, "tank"},
        type = {"filesystem", nil},
        guid = {"11", nil},
    }
    return ds("tank", system, user, {
        ds("tank/child", {
            sharenfs = {"rw=@10.0.0.0/8,\t\n", "tank/child"},
            type = {"filesystem", nil},
            guid = {"-1093669918614105072", nil},
        }, {{"u:x", "\128", "tank/child"}}),
        ds("tank/vol", {
            volsize = {1073741824, "tank/vol"},
            type = {"volume", nil},
            guid = {"33", nil},
        }),
    })
end

-- Many members, each with one 8191-byte user value of 0xff (the most a
-- user property's value may hold is below ZAP_MAXVALUELEN, 8192): the
-- hex path's cost in instructions per byte.
models.wide = function()
    local big = string.rep("\255", 8191)
    local children = {}
    for i = 1, 16 do
        children[#children + 1] = ds(string.format("tank/d%02d", i), {
            type = {"filesystem", nil}, guid = {tostring(100 + i), nil},
        }, {{"u:big", big, string.format("tank/d%02d", i)}})
    end
    return ds("tank", {type = {"filesystem", nil}, guid = {"11", nil}}, {}, children)
end

-- The same members with printable values of the same length.
models.wide_plain = function()
    local big = string.rep("x", 8191)
    local children = {}
    for i = 1, 16 do
        children[#children + 1] = ds(string.format("tank/d%02d", i), {
            type = {"filesystem", nil}, guid = {tostring(100 + i), nil},
        }, {{"u:big", big, string.format("tank/d%02d", i)}})
    end
    return ds("tank", {type = {"filesystem", nil}, guid = {"11", nil}}, {}, children)
end

-- A property that raises when read (zcp_get.c, zcp_handle_error: EIO).
models.raises = function()
    return ds("tank", {
        type = {"filesystem", nil}, guid = {"11", nil},
        bad = {"RAISE", "tank"},
    }, {}, {ds("tank/child", {type = {"filesystem", nil}, guid = {"22", nil}})})
end

local root = assert(models[scenario], "no such scenario")()
local by_name = {}
local function index(d)
    by_name[d.name] = d
    for _, c in ipairs(d.children) do index(c) end
end
index(root)

local snapshots = {}
local txg = 812
local guid_next = 9000

-- The zfs library, as zcp_iter.c, zcp_get.c and zcp_synctask.c give it.
local zfs = {list = {}, check = {}, sync = {}}
function zfs.exists(name) return by_name[name] ~= nil or snapshots[name] ~= nil end
function zfs.list.children(name)
    local d = assert(by_name[name], "no dataset " .. name)
    local i = 0
    return function()
        i = i + 1
        local c = d.children[i]
        return c and c.name
    end
end
function zfs.list.system_properties(name)
    local d = assert(by_name[name], "no dataset " .. name)
    local out = {}
    for k in pairs(d.system) do out[#out + 1] = k end
    table.sort(out)
    return out  -- an array of names, as zcp_nvlist_to_lua makes of booleans
end
function zfs.list.user_properties(name)
    local d = assert(by_name[name], "no dataset " .. name)
    local i = 0
    return function()
        i = i + 1
        local p = d.user[i]
        if p then return p[1], p[2], p[3] end
    end
end
function zfs.get_prop(name, property)
    local snap = snapshots[name]
    if snap then
        if property == "guid" then return snap.guid end
        if property == "createtxg" then return snap.txg end
        return nil
    end
    local d = assert(by_name[name], "no dataset " .. name)
    local p = d.system[property]
    if p == nil then return nil end
    if p[1] == "RAISE" then
        error("I/O error while retrieving property '" .. property .. "' on dataset '" .. name .. "'")
    end
    return p[1], p[2]
end
function zfs.check.snapshot(name)
    if snapshots[name] then return 17 end
    return 0
end
function zfs.sync.snapshot(name)
    if snapshots[name] then return 17 end
    guid_next = guid_next + 1
    snapshots[name] = {guid = guid_next, txg = txg}
    return 0
end

-- The sandbox's globals and nothing else.
local env = {}
for _, k in ipairs({"assert", "collectgarbage", "error", "getmetatable",
        "ipairs", "next", "pairs", "rawequal", "rawlen", "rawget", "rawset",
        "select", "setmetatable", "tonumber", "tostring", "type"}) do
    env[k] = _G[k]
end
env._G = env
env.coroutine = coroutine
local s = {}
for k, v in pairs(string) do if k ~= "dump" then s[k] = v end end
env.string = s
env.table = table
env.zfs = zfs
-- Anything else read from the environment is an error, as it would be
-- nil there: the first use of it fails the program.
setmetatable(env, {__index = function(_, k)
    return nil
end})

local chunk = assert(load(text, "=program", "t", env))
local count = 0
local co = coroutine.create(function()
    return chunk({argv = {"tank", "t", "skyzfs:assist"}})
end)
debug.sethook(co, function() count = count + 100 end, "", 100)
local ok, result = coroutine.resume(co)

local out = assert(io.open(out_json, "wb"))
local exp = assert(io.open(out_expected, "wb"))

if not ok then
    out:close()
    exp:write("error\t", tostring(result), "\n")
    exp:write("instructions\t", count, "\n")
    exp:write("taken\t", (function() local n = 0 for _ in pairs(snapshots) do n = n + 1 end return n end)(), "\n")
    exp:close()
    os.exit(1)
end

-- nvlist_print_json in the locale C.
local Stop = {}
local function jstring(w, v)
    w("\"")
    for i = 1, #v do
        local b = v:byte(i)
        if b >= 0x80 then error(Stop) end
        local c = string.char(b)
        if c == "\"" then w("\\\"")
        elseif c == "\n" then w("\\n")
        elseif c == "\r" then w("\\r")
        elseif c == "\\" then w("\\\\")
        elseif c == "\f" then w("\\f")
        elseif c == "\t" then w("\\t")
        elseif c == "\b" then w("\\b")
        elseif b <= 0x1f then w(string.format("\\u%04x", b))
        else w(c) end
    end
    w("\"")
end
local function jvalue(w, v)
    if type(v) == "table" then
        w("{")
        local first = true
        for k, x in pairs(v) do
            if not first then w(",") end
            first = false
            jstring(w, tostring(k))
            w(":")
            jvalue(w, x)
        end
        w("}")
    elseif type(v) == "number" then
        w(tostring(v))
    elseif type(v) == "boolean" then
        w(v and "true" or "false")
    else
        jstring(w, tostring(v))
    end
end
local parts = {}
local function w(x) parts[#parts + 1] = x end
local cut = false
local okj, err = pcall(function()
    w("{")
    jstring(w, "return")
    w(":")
    jvalue(w, result)
    w("}")
end)
if not okj then
    if err == Stop then cut = true else error(err) end
end
out:write(table.concat(parts))
if not cut then out:write("\n") end
out:close()

-- What the program was given, by bytes, for what it says is local.
local function bytes(v)
    v = tostring(v)
    local t = {}
    for i = 1, #v do t[#t + 1] = tostring(v:byte(i)) end
    return table.concat(t, ",")
end
local function walk(d)
    for k, p in pairs(d.system) do
        if p[2] == d.name and p[1] ~= nil then
            exp:write(d.name, "\t", k, "\t", bytes(p[1]), "\n")
        end
    end
    for _, p in ipairs(d.user) do
        if p[3] == d.name and p[1] ~= "skyzfs:assist" then
            exp:write(d.name, "\t", p[1], "\t", bytes(p[2]), "\n")
        end
    end
    for _, c in ipairs(d.children) do walk(c) end
end
walk(root)
exp:write("cut\t", tostring(cut), "\n")
exp:write("instructions\t", count, "\n")
exp:write("json_bytes\t", #table.concat(parts), "\n")
exp:close()
