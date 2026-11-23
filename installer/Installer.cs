using System;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Reflection;
using System.Windows.Forms;

[assembly: AssemblyTitle("GMO TempTools Setup")]
[assembly: AssemblyDescription("Installer for GMO TempTools")]
[assembly: AssemblyProduct("GMO TempTools")]
[assembly: AssemblyCompany("drifter1987")]
[assembly: AssemblyVersion("0.3.0.0")]
[assembly: AssemblyFileVersion("0.3.0.0")]
[assembly: AssemblyInformationalVersion("0.3.0")]

namespace GmoTempToolsSetup
{
    internal static class Program
    {
        [STAThread]
        private static void Main()
        {
            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);
            string[] args = Environment.GetCommandLineArgs();
            if (args.Length > 1 &&
                (string.Equals(args[1], "/S", StringComparison.OrdinalIgnoreCase) ||
                 string.Equals(args[1], "/silent", StringComparison.OrdinalIgnoreCase)))
            {
                Installer.InstallSilent();
                return;
            }
            Application.Run(new InstallerForm());
        }
    }

    internal static class Installer
    {
        internal static string DestinationDirectory()
        {
            return Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "Programs",
                "GMO TempTools");
        }

        internal static void InstallSilent()
        {
            string error;
            string target = Install(true, false, out error);
            if (target == null)
            {
                string log = Path.Combine(Path.GetTempPath(), "GMO-TempTools-Setup.log");
                File.WriteAllText(log, error == null ? "Unknown install error." : error);
                Environment.ExitCode = 1;
            }
        }

        internal static string Install(bool desktopShortcut, bool launch, out string error)
        {
            error = null;
            try
            {
                string directory = DestinationDirectory();
                string target = Path.Combine(directory, "gmo-temptools.exe");
                string temporary = target + ".new";
                Directory.CreateDirectory(directory);

                using (Stream resource = Assembly.GetExecutingAssembly()
                    .GetManifestResourceStream("GMOApp.exe"))
                {
                    if (resource == null)
                    {
                        throw new InvalidOperationException("The app payload is missing from this setup.");
                    }
                    using (FileStream output = new FileStream(
                        temporary, FileMode.Create, FileAccess.Write, FileShare.None))
                    {
                        resource.CopyTo(output);
                    }
                }

                if (File.Exists(target))
                {
                    File.Delete(target);
                }
                File.Move(temporary, target);

                try
                {
                    CreateShortcut(
                        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Programs),
                            "GMO TempTools.lnk"),
                        target,
                        directory);
                    if (desktopShortcut)
                    {
                        CreateShortcut(
                            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory),
                                "GMO TempTools.lnk"),
                            target,
                            directory);
                    }
                }
                catch
                {
                    // The app is installed even if COM shortcut creation is
                    // unavailable on a locked-down machine.
                }

                if (launch)
                {
                    ProcessStartInfo start = new ProcessStartInfo(target);
                    start.WorkingDirectory = directory;
                    start.UseShellExecute = true;
                    Process.Start(start);
                }
                return target;
            }
            catch (Exception failure)
            {
                error = failure.Message;
                return null;
            }
        }

        private static void CreateShortcut(string linkPath, string target, string workingDirectory)
        {
            object shell = Activator.CreateInstance(Type.GetTypeFromProgID("WScript.Shell"));
            object shortcut = shell.GetType().InvokeMember(
                "CreateShortcut",
                BindingFlags.InvokeMethod,
                null,
                shell,
                new object[] { linkPath });
            Type shortcutType = shortcut.GetType();
            shortcutType.InvokeMember("TargetPath", BindingFlags.SetProperty, null, shortcut, new object[] { target });
            shortcutType.InvokeMember("WorkingDirectory", BindingFlags.SetProperty, null, shortcut, new object[] { workingDirectory });
            shortcutType.InvokeMember("Description", BindingFlags.SetProperty, null, shortcut, new object[] { "GMO TempTools" });
            shortcutType.InvokeMember("Save", BindingFlags.InvokeMethod, null, shortcut, null);
        }
    }

    internal sealed class InstallerForm : Form
    {
        private readonly CheckBox desktopShortcut;
        private readonly CheckBox launchAfterInstall;
        private readonly Button installButton;
        private readonly Label status;
        private bool installing;

        public InstallerForm()
        {
            Text = "GMO TempTools Setup";
            ClientSize = new Size(520, 300);
            FormBorderStyle = FormBorderStyle.FixedDialog;
            MaximizeBox = false;
            MinimizeBox = false;
            StartPosition = FormStartPosition.CenterScreen;
            Font = new Font("Segoe UI", 9F);

            Label title = new Label();
            title.Text = "GMO TempTools";
            title.Font = new Font("Segoe UI", 20F, FontStyle.Bold);
            title.AutoSize = true;
            title.Location = new Point(28, 22);
            Controls.Add(title);

            Label subtitle = new Label();
            subtitle.Text = "One app for temporary mail, SMS and OTP. Version 0.3.0.";
            subtitle.ForeColor = Color.DimGray;
            subtitle.AutoSize = true;
            subtitle.Location = new Point(31, 62);
            Controls.Add(subtitle);

            Label destination = new Label();
            destination.Text = "Install for this Windows user:";
            destination.AutoSize = true;
            destination.Location = new Point(31, 103);
            Controls.Add(destination);

            Label path = new Label();
            path.Text = Installer.DestinationDirectory();
            path.AutoEllipsis = true;
            path.BorderStyle = BorderStyle.FixedSingle;
            path.BackColor = Color.White;
            path.Location = new Point(34, 124);
            path.Size = new Size(452, 24);
            path.TextAlign = ContentAlignment.MiddleLeft;
            Controls.Add(path);

            desktopShortcut = new CheckBox();
            desktopShortcut.Text = "Create a desktop shortcut";
            desktopShortcut.Checked = true;
            desktopShortcut.AutoSize = true;
            desktopShortcut.Location = new Point(34, 165);
            Controls.Add(desktopShortcut);

            launchAfterInstall = new CheckBox();
            launchAfterInstall.Text = "Launch GMO TempTools after installing";
            launchAfterInstall.Checked = true;
            launchAfterInstall.AutoSize = true;
            launchAfterInstall.Location = new Point(34, 190);
            Controls.Add(launchAfterInstall);

            status = new Label();
            status.Text = "Ready to install. No administrator rights are required.";
            status.ForeColor = Color.DimGray;
            status.AutoSize = true;
            status.Location = new Point(34, 222);
            Controls.Add(status);

            installButton = new Button();
            installButton.Text = "Install";
            installButton.Size = new Size(100, 32);
            installButton.Location = new Point(286, 252);
            installButton.Click += InstallClicked;
            Controls.Add(installButton);

            Button cancel = new Button();
            cancel.Text = "Cancel";
            cancel.Size = new Size(100, 32);
            cancel.Location = new Point(392, 252);
            cancel.Click += delegate { Close(); };
            Controls.Add(cancel);

            AcceptButton = installButton;
            CancelButton = cancel;
        }

        private void InstallClicked(object sender, EventArgs e)
        {
            if (installing)
            {
                return;
            }
            installing = true;
            installButton.Enabled = false;
            UseWaitCursor = true;
            status.Text = "Installing...";
            status.ForeColor = Color.DimGray;
            Application.DoEvents();

            string error;
            string target = Installer.Install(
                desktopShortcut.Checked,
                launchAfterInstall.Checked,
                out error);
            UseWaitCursor = false;
            if (target == null)
            {
                status.Text = "Installation failed.";
                status.ForeColor = Color.Firebrick;
                MessageBox.Show(this,
                    "Could not install GMO TempTools.\n\n" + (error ?? "Unknown error.") +
                    "\n\nIf the app is already running, close it and try again.",
                    "GMO TempTools Setup",
                    MessageBoxButtons.OK,
                    MessageBoxIcon.Error);
                installing = false;
                installButton.Enabled = true;
                return;
            }

            status.Text = "Installed.";
            status.ForeColor = Color.SeaGreen;
            MessageBox.Show(this,
                "GMO TempTools is installed.\n\n" + target +
                "\n\nOpen Settings in the app to see the installation key and local data paths.",
                "GMO TempTools Setup",
                MessageBoxButtons.OK,
                MessageBoxIcon.Information);
            Close();
        }
    }
}
