package handler

import (
	"encoding/json"
	"net/http"
)

// Status is what /healthz returns.
type Status struct {
	OK      bool   `json:"ok"`
	Version string `json:"version"`
}

func Health(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(Status{OK: true, Version: "1.2.3"})
}
