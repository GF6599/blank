package app

import (
	"go.uber.org/zap"
)

var log *zap.SugaredLogger

func init() {
	logger := zap.Must(zap.NewDevelopment())
	defer logger.Sync()
	log = logger.Sugar()
}
