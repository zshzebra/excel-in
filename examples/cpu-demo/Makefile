XLSX      := RISC-CPU.xlsx
ROM       := ROM.xlsx
ASM       := gradient.s
ASSEMBLER := assembler.py
DEF       := api.kdl
LIB       := libspreadsheet.so
HEADER    := libspreadsheet.h
COMPILER  := cargo run -p excel-in-compile --
OPT       := -O2

all: demo

rom: $(ASM)
	uv run --with openpyxl python3 $(ASSEMBLER) $(ASM) $(ROM)

$(LIB): $(XLSX) $(ROM) $(DEF)
	$(COMPILER) $(XLSX) --def $(DEF) -o $(LIB) $(OPT)

demo: main.c $(LIB)
	cc -o demo main.c -L. -lspreadsheet -lm -Wl,-rpath,'$$ORIGIN' -I. -include $(HEADER)

run: demo
	./demo

clean:
	rm -f demo $(LIB) $(HEADER) *.o

.PHONY: all rom run clean
