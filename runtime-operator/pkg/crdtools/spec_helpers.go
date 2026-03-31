package crdtools

import (
	"sort"

	corev1 "k8s.io/api/core/v1"
)

// MergeLabels merges multiple maps of labels into a single map.
func MergeLabels(lbls ...map[string]string) map[string]string {
	ret := make(map[string]string)

	for _, lbl := range lbls {
		for k, v := range lbl {
			ret[k] = v
		}
	}

	return ret
}

// MergeMounts merges multiple slices of VolumeMounts into a single slice.
func MergeMounts(mounts ...[]corev1.VolumeMount) []corev1.VolumeMount {
	var ret []corev1.VolumeMount

	for _, mnts := range mounts {
		ret = append(ret, mnts...)
	}

	return ret
}

// MergeEnvFromSource merges multiple slices of EnvFromSource into a single slice.
func MergeEnvFromSource(srcs ...[]corev1.EnvFromSource) []corev1.EnvFromSource {
	ret := make([]corev1.EnvFromSource, 0)

	for _, evs := range srcs {
		ret = append(ret, evs...)
	}

	return ret
}

// MergeEnvVar merges multiple slices of EnvVar into a single slice.
func MergeEnvVar(envs ...[]corev1.EnvVar) []corev1.EnvVar {
	idx := make(map[string]corev1.EnvVar)

	for _, evs := range envs {
		for _, ev := range evs {
			idx[ev.Name] = ev
		}
	}

	keys := make([]string, 0, len(idx))
	for k := range idx {
		keys = append(keys, k)
	}
	sort.Strings(keys)

	ret := make([]corev1.EnvVar, len(keys))
	for i, k := range keys {
		ret[i] = idx[k]
	}

	return ret
}

// Int64Ptr returns a pointer to an int64.
func Int64Ptr(i int64) *int64 {
	return &i
}

// Int32Ptr returns a pointer to an int32.
func Int32Ptr(i int32) *int32 {
	return &i
}

// BoolPtr returns a pointer to a bool.
func BoolPtr(t bool) *bool {
	return &t
}
