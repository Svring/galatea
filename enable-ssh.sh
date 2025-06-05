#!/bin/bash

# This script enables password-based SSH login and sets a password for the 'devbox' user.

# Ensure it's run with sudo/root privileges
if [ "$EUID" -ne 0 ]; then
  echo "Please run this script as root (e.g., with sudo)"
  exit 1
fi

SSH_CONFIG="/etc/ssh/sshd_config"
USERNAME="devbox"
PASSWORD="123"  # <-- Change this

# Enable PasswordAuthentication yes
if grep -q "^#*PasswordAuthentication" "$SSH_CONFIG"; then
  sed -i 's/^#*PasswordAuthentication.*/PasswordAuthentication yes/' "$SSH_CONFIG"
else
  echo "PasswordAuthentication yes" >> "$SSH_CONFIG"
fi

# Enable UsePAM yes
if grep -q "^#*UsePAM" "$SSH_CONFIG"; then
  sed -i 's/^#*UsePAM.*/UsePAM yes/' "$SSH_CONFIG"
else
  echo "UsePAM yes" >> "$SSH_CONFIG"
fi

# Set password for devbox
echo "Setting password for $USERNAME..."
echo "$USERNAME:$PASSWORD" | chpasswd || {
  echo "Failed to set password for $USERNAME"
  exit 1
}

# Restart SSH service
echo "Restarting SSH service..."
if systemctl restart sshd 2>/dev/null; then
  echo "SSH service restarted successfully (systemd)"
elif service ssh restart 2>/dev/null; then
  echo "SSH service restarted successfully (SysV)"
else
  echo "Warning: SSH service could not be restarted. Please restart manually if needed."
fi

echo "✅ SSH password login enabled and password set for user '$USERNAME'."
