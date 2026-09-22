package crdtools

import "testing"

func TestCoalesce(t *testing.T) {
	tests := []struct {
		name string
		args []string
		want string
	}{
		{
			name: "empty",
			args: []string{""},
			want: "",
		},
		{
			name: "first set wins",
			args: []string{"a", "b"},
			want: "a",
		},
		{
			name: "skips one empty",
			args: []string{"", "b"},
			want: "b",
		},
		{
			name: "skips two empty",
			args: []string{"", "", "c"},
			want: "c",
		},
		{
			name: "skips three empty",
			args: []string{"", "", "", "d"},
			want: "d",
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := Coalesce(tt.args...); got != tt.want {
				t.Errorf("Coalesce() = %v, want %v", got, tt.want)
			}
		})
	}
}
