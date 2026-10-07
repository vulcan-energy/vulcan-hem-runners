run-all-example-files:
	@for f in examples/input/future_homes_standard/*.json; do \
  		case "$$f" in \
              examples/input/future_homes_standard/DESN*) continue ;; \
          esac; \
   		echo "Running on $$f"; \
        out="$$(cargo run --release --features="clap indicatif" -- $$f 2>&1)"; \
        echo "$$out" | grep -qi "error" && echo "❗ Error detected in $$f"; \
        echo ""; \
	done
