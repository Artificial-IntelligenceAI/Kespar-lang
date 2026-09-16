-- same as ../sieve.kpls: n from stdin (2..20000), 30 reps
local n = tonumber(io.read("l"))
local count = 0
for rep = 1, 30 do
  count = 0
  local flags = {}
  for i = 0, 20000 do flags[i] = true end
  for i = 2, n do
    if flags[i] then
      count = count + 1
      local j = i * 2
      while j <= n do
        flags[j] = false
        j = j + i
      end
    end
  end
end
print(count)
