// Package cmd contains the root command for starting the gs application
package cmd

import (
	"fmt"
	"os"

	gshred "gshred/app"

	"github.com/spf13/cobra"
)

var rootCmd = &cobra.Command{
	Use:   "gshred [file|dir]...",
	Short: "Concurrnet Shred command implemented in go.",
	Long:  "Will use the bash 'shred -n 10 -uvz' command to shred any files or dir provided to gshred.",
	Run: func(cmd *cobra.Command, args []string) {
		if len(args) == 0 {
			cmd.Help()
			return
		}
		gshred.Gshred(args)
	},
}

func Execute() {
	if err := rootCmd.Execute(); err != nil {
		fmt.Println(err)
		os.Exit(1)
	}
}
