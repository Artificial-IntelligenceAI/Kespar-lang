-- same as ../sum_squares_known.kpls: n is a constant
local n = 1000
local total = 0
for rep = 1, 5000 do
  total = 0
  for i = 1, n do
    total = total + i * i
  end
end
print(total)
