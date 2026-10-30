# Build, test and clean. Recipes need tabs.
CC ?= cc
CFLAGS := -O2 -Wall -Wextra
SRC := $(wildcard src/*.c)
OBJ := $(SRC:.c=.o)

.PHONY: all test clean

all: app

app: $(OBJ)
	$(CC) $(CFLAGS) -o $@ $^

%.o: %.c
	$(CC) $(CFLAGS) -c -o $@ $<

test: app
	./app --self-test

clean:
	rm -f app $(OBJ)
