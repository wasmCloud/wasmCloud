# provider/httpserver/Makefile

.PHONY: httpserver keyvalue build help
CARGO ?= cargo --color always
CAPID = wasmcloud:httpserver
NAME = "HttpServer Provider (Go)"

DIST_BIN = build/httpserver-linux
DIST_PAR = build/httpserver.par.gz
DIST_EXS = build/exs.out

all: $(DIST_BIN) $(DIST_PAR) $(DIST_EXS)

.ONESHELL:
$(DIST_EXS): $(DIST_BIN) $(DIST_PAR)
	@mkdir -p $(dir $@)
	@cat <<- EOF > $@
	%{
		:name => $(NAME),
		:path => "$(abspath $(DIST_BIN))",
		:key => "$(shell wash par inspect $(DIST_PAR) -o json | jq -r ".service")",
		:link => "default",
		:contract => "$(CAPID)",
		:params => %{ PORT: 8080 },
	},
	EOF


$(DIST_BIN): go/*.mod go/*.go Makefile
	@mkdir -p $(dir $@)
	cd go && \
	  GOOS=linux go build -o ../$@


$(DIST_PAR): $(DIST_BIN)
	@mkdir -p $(dir $@)
	wash par create --arch x86_64-linux \
		--binary $(DIST_BIN) \
		--capid wasmcloud:httpserver \
		--name "HTTPServer (Go)" \
		--vendor "wasmCloud" \
		--version 0.13.0 \
		--revision 3 \
		--destination $(DIST_PAR) \
		--compress

#	wash par insert httpserver.par.gz \
#		--arch x86_64-linux \
#		--binary httpserver/go/httpserver-linux

inspect claims: $(DIST_PAR)
	wash par inspect $(DIST_PAR)

clean:
	rm -f $(DIST_BIN) $(DIST_PAR) $(DIST_EXS)
