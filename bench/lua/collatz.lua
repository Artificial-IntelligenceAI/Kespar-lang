-- same as ../collatz.kpls: limit from stdin (1..100), 300 reps (no `//`: LuaJIT is Lua 5.1)
local limit = tonumber(io.read("l"))
local longest = 0
for rep = 1, 300 do
  for start = 1, limit do
    local x = start
    local steps = 0
    while x ~= 1 do
      if x % 2 == 0 then x = (x - x % 2) / 2 else x = 3 * x + 1 end
      steps = steps + 1
    end
    if steps > longest then longest = steps end
  end
end
print(longest)
